//! The two-voice "podcast" recipe (0.13 stretch goal, #259, P17): pure
//! script parsing and chunking. The *Podcast script* recipe
//! ([`crate::recipes::PODCAST_SCRIPT_ID`]) asks the LLM for a two-host
//! dialogue, one line per turn, each starting with `Host A:` or `Host B:`
//! (see [`crate::recipes::PODCAST_SCRIPT_PROMPT`]); this module turns that
//! markdown into [`Turn`]s and, per [`crate::tts::text`]'s language rules,
//! into per-host [`Chunk`]s ready to speak.
//!
//! Rendering (in [`super::read_aloud`]) alternates two distinct **built-in**
//! Pocket voices for the item's language — never cloned, never a third
//! voice, and only voices whose recordings allow commercial use
//! ([`super::catalog`] already lists nothing else). The result is one
//! marked Ogg Opus file, same as ordinary read aloud, with both voice ids
//! recorded in its `synthetic:` frontmatter
//! ([`crate::archive::speech::SpeechInfo::voice_b`]) and Ogg comments
//! (`TTS_VOICE_B`, [`super::marking::Provenance::voice_b`]) — never a
//! cloned voice, never a name mistaken for a recording (P24, #257).
//!
//! The document this reads is always [`SCRIPT_FILE`], the companion file
//! the *Podcast script* recipe writes — [`super::read_aloud::run`]
//! dispatches to the two-voice path only for that exact name; every other
//! document keeps the ordinary single-narrator path.

use super::catalog::{Language, Voice};
use super::text::{self, Chunk, Lang, PrepOptions};

/// The companion document the *Podcast script* recipe writes
/// (`companion_file_name` of [`crate::recipes::PODCAST_SCRIPT_ID`] — a test
/// keeps the two in step).
pub const SCRIPT_FILE: &str = "podcast-script.md";

/// One of the two hosts of a podcast script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Host {
    A,
    B,
}

/// One turn of the dialogue: everything one host says until the other
/// host's tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub host: Host,
    pub text: String,
}

/// `*`/`_` markdown emphasis trimmed off both ends, then whitespace. Pure.
fn strip_emphasis(s: &str) -> &str {
    s.trim_matches(|c: char| c == '*' || c == '_').trim()
}

/// A line's host tag (`Host A:`, `A:`, case-insensitive, markdown emphasis
/// allowed around it) and what follows it, if any. Pure.
fn split_tag(line: &str) -> Option<(Host, &str)> {
    let stripped = strip_emphasis(line);
    let lower = stripped.to_ascii_lowercase();
    const TAGS: &[(&str, Host)] = &[
        ("host a:", Host::A),
        ("host a :", Host::A),
        ("host b:", Host::B),
        ("host b :", Host::B),
        ("a:", Host::A),
        ("b:", Host::B),
    ];
    for (prefix, host) in TAGS {
        if lower.starts_with(prefix) {
            // The prefix is ASCII, so its byte length matches on the
            // original (unlowered) string.
            return Some((*host, strip_emphasis(&stripped[prefix.len()..])));
        }
    }
    None
}

/// The dialogue turns of a *Podcast script* document: every line starting
/// with a host tag opens a new turn; a line without one continues the
/// current turn (the model sometimes wraps a line); blank lines, headings
/// and anything before the first tag are dropped. Turns left empty (a tag
/// with nothing after it and no continuation) are dropped too. Pure.
pub fn parse_script(markdown: &str) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    for raw in markdown.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((host, rest)) = split_tag(line) {
            turns.push(Turn {
                host,
                text: rest.to_string(),
            });
        } else if !line.starts_with('#') {
            if let Some(last) = turns.last_mut() {
                if !last.text.is_empty() {
                    last.text.push(' ');
                }
                last.text.push_str(strip_emphasis(line));
            }
        }
    }
    turns.retain(|t| !t.text.trim().is_empty());
    turns
}

/// `turns` as chunks ready to speak, grouped by contiguous run of the same
/// host (so rendering switches voice once per run, not once per chunk).
/// Each turn's text goes through [`text::prepare`] on its own, so a long
/// turn is still split at [`text::DEFAULT_MAX_CHUNK_CHARS`]. Pure.
pub fn chunk_script(turns: &[Turn], lang: Lang) -> Vec<(Host, Vec<Chunk>)> {
    let mut out: Vec<(Host, Vec<Chunk>)> = Vec::new();
    for t in turns {
        let chunks = text::prepare(&t.text, lang, &PrepOptions::default());
        if chunks.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((host, cs)) if *host == t.host => cs.extend(chunks),
            _ => out.push((t.host, chunks)),
        }
    }
    out
}

/// A stable hash of the speakable script (host + text of every turn), for
/// the "out of date" check ([`super::read_aloud::statuses`]) — a `stale`
/// speech file is one whose script changed since it was read. Pure.
pub fn script_hash(turns: &[Turn]) -> String {
    let mut h = String::from("podcast-script v1\n");
    for t in turns {
        h.push_str(match t.host {
            Host::A => "A|",
            Host::B => "B|",
        });
        h.push_str(&t.text);
        h.push('\n');
    }
    crate::archive::store::sha256_hex(h.as_bytes())
}

/// Host B's voice: the first of `lang`'s voices that isn't `primary`
/// (Host A's) — two *different* built-in voices, never the same one twice.
/// Falls back to `primary` only when the language has just one voice
/// (never true today: every [`super::catalog`] language lists several).
/// Deterministic. Pure.
pub fn second_voice(lang: &'static Language, primary: &'static Voice) -> &'static Voice {
    lang.voices
        .iter()
        .find(|v| v.id != primary.id)
        .unwrap_or(primary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::catalog;

    #[test]
    fn parses_alternating_hosts_and_drops_noise() {
        let md = "# Podcast\n\nHost A: Welcome to the show.\nHost B: Glad to be here.\n\n**Host A:** Let's dive in.\nB: Sure thing.\n";
        let turns = parse_script(md);
        assert_eq!(
            turns,
            vec![
                Turn {
                    host: Host::A,
                    text: "Welcome to the show.".into()
                },
                Turn {
                    host: Host::B,
                    text: "Glad to be here.".into()
                },
                Turn {
                    host: Host::A,
                    text: "Let's dive in.".into()
                },
                Turn {
                    host: Host::B,
                    text: "Sure thing.".into()
                },
            ]
        );
    }

    #[test]
    fn a_wrapped_line_continues_the_current_turn() {
        let md = "Host A: This is a long line\nthat wraps onto the next one.\nHost B: Right.";
        let turns = parse_script(md);
        assert_eq!(turns.len(), 2);
        assert_eq!(
            turns[0].text,
            "This is a long line that wraps onto the next one."
        );
    }

    #[test]
    fn text_before_the_first_tag_and_empty_turns_are_dropped() {
        let md = "Some preamble the model added.\nHost A:\nHost B: Only this line matters.";
        let turns = parse_script(md);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].host, Host::B);
    }

    #[test]
    fn a_script_with_no_tags_parses_to_nothing() {
        assert!(parse_script("Just a plain paragraph, no hosts at all.").is_empty());
        assert!(parse_script("").is_empty());
    }

    #[test]
    fn tag_matching_is_case_insensitive_and_not_fooled_by_names() {
        let md = "host a: lower case works\nHOST B: upper case too\nBen: this is not a tag, so it continues Host B's turn";
        let turns = parse_script(md);
        assert_eq!(turns.len(), 2);
        assert!(turns[1].text.contains("Ben: this is not a tag"));
    }

    #[test]
    fn chunking_groups_contiguous_turns_by_host_and_splits_long_ones() {
        let turns = vec![
            Turn {
                host: Host::A,
                text: "One.".into(),
            },
            Turn {
                host: Host::A,
                text: "Two.".into(),
            },
            Turn {
                host: Host::B,
                text: "Three.".into(),
            },
        ];
        let groups = chunk_script(&turns, Lang::from_code("en"));
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, Host::A);
        assert_eq!(groups[0].1.len(), 2, "A's two turns stay separate chunks");
        assert_eq!(groups[1].0, Host::B);

        let long = "word ".repeat(100);
        let turns = vec![Turn {
            host: Host::A,
            text: long,
        }];
        let groups = chunk_script(&turns, Lang::from_code("en"));
        assert_eq!(groups.len(), 1);
        assert!(groups[0].1.len() > 1, "a long turn is split into chunks");
    }

    #[test]
    fn the_hash_follows_host_and_text_only() {
        let a = vec![Turn {
            host: Host::A,
            text: "Hi".into(),
        }];
        let b = vec![Turn {
            host: Host::B,
            text: "Hi".into(),
        }];
        assert_ne!(
            script_hash(&a),
            script_hash(&b),
            "the host is part of the hash"
        );
        assert_eq!(script_hash(&a), script_hash(&a.clone()), "deterministic");
    }

    #[test]
    fn second_voice_is_always_different_from_the_first() {
        for lang in catalog::LANGUAGES {
            let primary = lang.voice(lang.default_voice).unwrap();
            let b = second_voice(lang, primary);
            assert_ne!(b.id, primary.id, "{}", lang.code);
            assert!(lang.voices.len() > 1, "{} needs a second voice", lang.code);
        }
    }

    #[test]
    fn the_script_document_name_is_a_speech_file_name() {
        assert!(crate::archive::speech::is_speech_file_name(
            &crate::archive::speech::speech_file_name(SCRIPT_FILE).unwrap()
        ));
    }
}
