//! Subtitles (0.9, #133): SRT and WebVTT built from `segments.json`
//! (plan §4.2, §5). Pure — the sidecar file and the exports live in
//! [`super::export`].
//!
//! - Each cue holds at most [`MAX_LINES`] rows of about [`MAX_LINE_CHARS`]
//!   characters and lasts at most about [`MAX_CUE_MS`]; cues are split on
//!   word timings, at a long pause, or after a sentence once a row is full.
//! - The text is the cleaned `text` of each segment. Word timings belong to
//!   the raw STT words: when the cleaned words match them one to one they
//!   are used as they are, otherwise the cleaned words are spread over the
//!   raw timeline in proportion to their length. Segments without timings
//!   (or with estimated ones, `words_estimated`) work the same way.
//! - Segments whose STT failed (`stt_error`) or whose text is empty are
//!   skipped.
//! - With speakers, the first cue of each speaker turn starts with the
//!   speaker's label (`Anna: …`).
//! - Cues never overlap, and timestamps only move forward.

use super::types::{Segment, SegmentsFile, Word};

/// Characters per subtitle row (the usual broadcast guideline).
pub const MAX_LINE_CHARS: usize = 42;
/// Rows per cue.
pub const MAX_LINES: usize = 2;
/// Longest a cue stays on screen.
pub const MAX_CUE_MS: u64 = 7_000;
/// A pause at least this long between two words ends the cue.
pub const MAX_GAP_MS: u64 = 1_500;
/// Shortest a cue stays on screen, when the silence after it allows.
pub const MIN_DISPLAY_MS: u64 = 1_000;
/// When two cues overlap (two people talking at once), the first is cut
/// where the second starts only if it keeps at least this long; otherwise
/// the second waits for the first to end.
const MIN_CUT_MS: u64 = 500;

/// One subtitle: a time range and its rows of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub lines: Vec<String>,
}

/// A cleaned word placed on the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Timed {
    w: String,
    start: u64,
    end: u64,
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// "-->" would end a cue's timing line early in both formats.
fn safe_token(w: &str) -> String {
    w.replace("-->", "->")
}

/// The raw words of `seg`, ordered and inside the segment (words written by
/// hand or by an older engine may not be).
fn raw_timeline(seg: &Segment) -> Vec<Word> {
    let lo = seg.start_ms;
    let hi = seg.end_ms.max(lo);
    let mut last = lo;
    seg.words
        .iter()
        .filter(|w| !w.w.trim().is_empty())
        .map(|w| {
            let start = w.start_ms.clamp(last, hi);
            let end = w.end_ms.clamp(start, hi);
            last = end;
            Word {
                w: w.w.trim().to_string(),
                start_ms: start,
                end_ms: end,
            }
        })
        .collect()
}

/// Time at `x` characters into a raw timeline whose words have the given
/// character `weights`. At a word boundary, `at_end` picks the end of the
/// word before (for a cleaned word's end), otherwise the start of the word
/// after (for a cleaned word's start).
fn time_at(raw: &[Word], weights: &[f64], x: f64, at_end: bool) -> u64 {
    let mut acc = 0.0;
    for (i, (w, &weight)) in raw.iter().zip(weights).enumerate() {
        let next = acc + weight;
        let last = i + 1 == raw.len();
        let inside = if at_end { x <= next } else { x < next };
        if inside || last {
            let frac = ((x - acc) / weight).clamp(0.0, 1.0);
            let span = w.end_ms.saturating_sub(w.start_ms) as f64;
            return w.start_ms + (frac * span).round() as u64;
        }
        acc = next;
    }
    0
}

/// The cleaned words of `seg` with timings (see the module docs).
fn timed_words(seg: &Segment) -> Vec<Timed> {
    let tokens: Vec<String> = seg.text.split_whitespace().map(safe_token).collect();
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut raw = raw_timeline(seg);
    if raw.len() == tokens.len() {
        return tokens
            .into_iter()
            .zip(raw)
            .map(|(w, r)| Timed {
                w,
                start: r.start_ms,
                end: r.end_ms,
            })
            .collect();
    }
    if raw.is_empty() {
        // No timings at all: the whole segment is one "word" to share.
        raw.push(Word {
            w: seg.text.clone(),
            start_ms: seg.start_ms,
            end_ms: seg.end_ms.max(seg.start_ms),
        });
    }
    let weight = |w: &str| chars(w).max(1) as f64;
    let raw_weights: Vec<f64> = raw.iter().map(|w| weight(&w.w)).collect();
    let raw_total: f64 = raw_weights.iter().sum();
    let tok_total: f64 = tokens.iter().map(|t| weight(t)).sum();
    let mut acc = 0.0;
    tokens
        .into_iter()
        .map(|w| {
            let a = acc / tok_total * raw_total;
            acc += weight(&w);
            let b = acc / tok_total * raw_total;
            let start = time_at(&raw, &raw_weights, a, false);
            let end = time_at(&raw, &raw_weights, b, true).max(start);
            Timed { w, start, end }
        })
        .collect()
}

/// Lay `tokens` out in at most [`MAX_LINES`] rows of at most
/// [`MAX_LINE_CHARS`], as balanced as possible. `None` when they don't fit.
fn layout(tokens: &[&str]) -> Option<Vec<String>> {
    let joined = tokens.join(" ");
    if chars(&joined) <= MAX_LINE_CHARS {
        return Some(vec![joined]);
    }
    let mut best: Option<(usize, Vec<String>)> = None;
    for k in 1..tokens.len() {
        let (a, b) = (tokens[..k].join(" "), tokens[k..].join(" "));
        let (la, lb) = (chars(&a), chars(&b));
        if la > MAX_LINE_CHARS || lb > MAX_LINE_CHARS {
            continue;
        }
        let worst = la.max(lb);
        if best.as_ref().is_none_or(|(w, _)| worst < *w) {
            best = Some((worst, vec![a, b]));
        }
    }
    best.map(|(_, lines)| lines)
}

/// Rows for a cue that can't be laid out within the limits — only a word
/// longer than a row (a URL) gets here: greedy rows, the long word alone.
fn force_layout(tokens: &[&str]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for t in tokens {
        match lines.last_mut() {
            Some(l) if chars(l) + 1 + chars(t) <= MAX_LINE_CHARS => {
                l.push(' ');
                l.push_str(t);
            }
            _ => lines.push((*t).to_string()),
        }
    }
    lines
}

fn ends_sentence(w: &str) -> bool {
    w.trim_end_matches(['"', '\'', ')', '»', '”', '’'])
        .ends_with(['.', '!', '?', '…'])
}

/// Cues of one segment's words; `prefix` (`Anna:`) opens the first one.
fn segment_cues(words: &[Timed], prefix: Option<&str>) -> Vec<Cue> {
    let mut cues = Vec::new();
    let mut cur: Vec<&Timed> = Vec::new();
    let mut prefix = prefix;
    let tokens = |cur: &[&Timed], prefix: Option<&str>, extra: Option<&Timed>| -> Vec<String> {
        prefix
            .map(str::to_string)
            .into_iter()
            .chain(cur.iter().map(|t| t.w.clone()))
            .chain(extra.map(|t| t.w.clone()))
            .collect()
    };
    let flush = |cur: &mut Vec<&Timed>, prefix: &mut Option<&str>, cues: &mut Vec<Cue>| {
        if cur.is_empty() {
            return;
        }
        let toks = tokens(cur, *prefix, None);
        let refs: Vec<&str> = toks.iter().map(String::as_str).collect();
        let lines = layout(&refs).unwrap_or_else(|| force_layout(&refs));
        let start = cur[0].start;
        let end = cur[cur.len() - 1].end.max(start).min(start + MAX_CUE_MS);
        cues.push(Cue {
            start_ms: start,
            end_ms: end,
            lines,
        });
        cur.clear();
        *prefix = None;
    };
    for w in words {
        if let (Some(first), Some(last)) = (cur.first(), cur.last()) {
            let toks = tokens(&cur, prefix, Some(w));
            let refs: Vec<&str> = toks.iter().map(String::as_str).collect();
            let too_long = layout(&refs).is_none();
            let too_slow = w.end.saturating_sub(first.start) > MAX_CUE_MS;
            let pause = w.start.saturating_sub(last.end) >= MAX_GAP_MS;
            // After a sentence, a cue that already fills a row ends there
            // rather than carrying the start of the next sentence.
            let so_far: usize = toks[..toks.len() - 1].iter().map(|t| chars(t) + 1).sum();
            let sentence = ends_sentence(&last.w) && so_far > MAX_LINE_CHARS;
            if too_long || too_slow || pause || sentence {
                flush(&mut cur, &mut prefix, &mut cues);
            }
        }
        cur.push(w);
    }
    flush(&mut cur, &mut prefix, &mut cues);
    cues
}

/// Order the cues and make the timeline well-formed: no overlaps, times
/// moving forward, every cue with a positive duration, short cues held a
/// little longer when the silence after them allows.
fn resolve(mut cues: Vec<Cue>) -> Vec<Cue> {
    cues.sort_by_key(|c| c.start_ms);
    let mut out: Vec<Cue> = Vec::with_capacity(cues.len());
    for mut c in cues {
        c.end_ms = c.end_ms.max(c.start_ms + 1);
        if let Some(p) = out.last_mut() {
            if c.start_ms < p.end_ms {
                if c.start_ms >= p.start_ms + MIN_CUT_MS {
                    p.end_ms = c.start_ms;
                } else {
                    let d = c.end_ms - c.start_ms;
                    c.start_ms = p.end_ms;
                    c.end_ms = c.start_ms + d;
                }
            }
        }
        out.push(c);
    }
    for i in 0..out.len() {
        let next = out.get(i + 1).map(|n| n.start_ms).unwrap_or(u64::MAX);
        let c = &mut out[i];
        let want = c.end_ms.max(c.start_ms + MIN_DISPLAY_MS);
        c.end_ms = want.min(next).max(c.end_ms);
    }
    out
}

/// The label of `seg`'s speaker, if the document knows it.
fn speaker_label(file: &SegmentsFile, seg: &Segment) -> Option<String> {
    let id = seg.speaker_id.as_deref()?;
    let sp = file.speakers.iter().find(|s| s.id == id)?;
    let label = sp.label.split_whitespace().collect::<Vec<_>>().join(" ");
    (!label.is_empty()).then_some(label)
}

/// Every cue of a document, in order (see the module docs).
pub fn build_cues(file: &SegmentsFile) -> Vec<Cue> {
    let mut segs: Vec<&Segment> = file
        .segments
        .iter()
        .filter(|s| s.stt_error.is_none() && !s.text.trim().is_empty())
        .collect();
    segs.sort_by_key(|s| s.start_ms);
    let with_speakers = !file.speakers.is_empty();
    let mut last_speaker: Option<String> = None;
    let mut cues = Vec::new();
    for seg in segs {
        let label = if with_speakers {
            speaker_label(file, seg)
        } else {
            None
        };
        let prefix = match &label {
            Some(l) if last_speaker.as_ref() != Some(l) => Some(format!("{l}:")),
            _ => None,
        };
        last_speaker = label;
        cues.extend(segment_cues(&timed_words(seg), prefix.as_deref()));
    }
    resolve(cues)
}

/// `HH:MM:SS<sep>mmm` (hours keep growing past 99).
fn timestamp(ms: u64, sep: char) -> String {
    let s = ms / 1000;
    format!(
        "{:02}:{:02}:{:02}{sep}{:03}",
        s / 3600,
        (s / 60) % 60,
        s % 60,
        ms % 1000
    )
}

/// SubRip: numbered cues, `00:00:01,500 --> 00:00:03,000`.
pub fn to_srt(cues: &[Cue]) -> String {
    cues.iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}\n{} --> {}\n{}\n\n",
                i + 1,
                timestamp(c.start_ms, ','),
                timestamp(c.end_ms, ','),
                c.lines.join("\n")
            )
        })
        .collect()
}

/// WebVTT cue text: `&`, `<` and `>` are markup there.
fn vtt_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// WebVTT: the `WEBVTT` header, then `00:00:01.500 --> 00:00:03.000` cues.
pub fn to_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for c in cues {
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            timestamp(c.start_ms, '.'),
            timestamp(c.end_ms, '.'),
            c.lines
                .iter()
                .map(|l| vtt_escape(l))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::types::DocSpeaker;

    fn word(w: &str, s: u64, e: u64) -> Word {
        Word {
            w: w.into(),
            start_ms: s,
            end_ms: e,
        }
    }

    /// A segment whose raw words are `text`'s, one every `step` ms.
    fn seg(id: u32, start: u64, text: &str, step: u64) -> Segment {
        let words: Vec<Word> = text
            .split_whitespace()
            .enumerate()
            .map(|(i, w)| {
                let s = start + i as u64 * step;
                word(w, s, s + step - 50)
            })
            .collect();
        let end = words.last().map(|w| w.end_ms).unwrap_or(start);
        Segment {
            id,
            start_ms: start,
            end_ms: end,
            raw: text.into(),
            text: text.into(),
            words,
            ..Default::default()
        }
    }

    fn file(segments: Vec<Segment>) -> SegmentsFile {
        SegmentsFile {
            segments,
            ..Default::default()
        }
    }

    fn speaker(id: &str, label: &str) -> DocSpeaker {
        DocSpeaker {
            id: id.into(),
            label: label.into(),
            ..Default::default()
        }
    }

    fn assert_well_formed(cues: &[Cue]) {
        let mut last_end = 0;
        for c in cues {
            assert!(c.start_ms >= last_end, "overlap or going back: {cues:#?}");
            assert!(c.end_ms > c.start_ms, "empty cue: {c:?}");
            assert!(!c.lines.is_empty() && c.lines.len() <= MAX_LINES, "{c:?}");
            last_end = c.end_ms;
        }
    }

    #[test]
    fn golden_srt_and_vtt_for_a_short_transcription() {
        let f = file(vec![
            seg(0, 1_000, "Buongiorno a tutti.", 400),
            seg(1, 4_000, "Oggi parliamo di perché è così.", 300),
        ]);
        let cues = build_cues(&f);
        assert_eq!(
            to_srt(&cues),
            "1\n00:00:01,000 --> 00:00:02,150\nBuongiorno a tutti.\n\n\
             2\n00:00:04,000 --> 00:00:05,750\nOggi parliamo di perché è così.\n\n"
        );
        assert_eq!(
            to_vtt(&cues),
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.150\nBuongiorno a tutti.\n\n\
             00:00:04.000 --> 00:00:05.750\nOggi parliamo di perché è così.\n\n"
        );
    }

    #[test]
    fn timestamps_have_hours_minutes_seconds_and_millis() {
        assert_eq!(timestamp(0, ','), "00:00:00,000");
        assert_eq!(timestamp(3_723_045, ','), "01:02:03,045");
        assert_eq!(timestamp(59_999, '.'), "00:00:59.999");
        assert_eq!(timestamp(100 * 3_600_000, '.'), "100:00:00.000");
    }

    #[test]
    fn long_text_wraps_into_two_rows_of_at_most_42_characters() {
        // 15 words at 300 ms: 4.5 s, well under the duration cap.
        let text = "questa è una frase piuttosto lunga che deve andare su due righe \
                    senza superare il limite di caratteri";
        let cues = build_cues(&file(vec![seg(0, 0, text, 300)]));
        assert_well_formed(&cues);
        for c in &cues {
            for l in &c.lines {
                assert!(chars(l) <= MAX_LINE_CHARS, "{l:?} is {} chars", chars(l));
            }
        }
        // Nothing lost, nothing added.
        let all: Vec<String> = cues.iter().flat_map(|c| c.lines.clone()).collect();
        assert_eq!(
            all.join(" ").split_whitespace().collect::<Vec<_>>(),
            text.split_whitespace().collect::<Vec<_>>()
        );
        // The first cue uses both rows, balanced.
        assert_eq!(cues[0].lines.len(), 2);
        let (a, b) = (chars(&cues[0].lines[0]), chars(&cues[0].lines[1]));
        assert!(a.abs_diff(b) <= 12, "{:?}", cues[0].lines);
    }

    #[test]
    fn slow_speech_splits_at_seven_seconds() {
        // Short words, one per second: they'd fit the rows, not the time.
        let text = "uno due tre quattro cinque sei sette otto nove dieci undici dodici";
        let cues = build_cues(&file(vec![seg(0, 0, text, 1_000)]));
        assert_well_formed(&cues);
        assert!(cues.len() >= 2, "{cues:#?}");
        for c in &cues {
            assert!(c.end_ms - c.start_ms <= MAX_CUE_MS, "{c:?}");
        }
        assert_eq!(cues[0].start_ms, 0);
        assert_eq!(cues[0].lines, vec!["uno due tre quattro cinque sei sette"]);
        assert_eq!(cues[1].start_ms, 7_000);
    }

    #[test]
    fn a_long_pause_ends_the_cue() {
        let mut s = seg(0, 0, "prima dopo", 300);
        s.words[1] = word("dopo", 3_000, 3_400);
        s.end_ms = 3_400;
        let cues = build_cues(&file(vec![s]));
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].lines, vec!["prima"]);
        assert_eq!(cues[1].start_ms, 3_000);
    }

    #[test]
    fn a_full_row_ends_the_cue_at_a_sentence_end() {
        let text = "Questa prima frase occupa quasi una riga intera. Poi ne arriva un'altra.";
        let cues = build_cues(&file(vec![seg(0, 0, text, 250)]));
        // The first sentence takes both rows; the second starts a new cue
        // even though its first words would still fit.
        assert_eq!(
            cues[0].lines,
            vec!["Questa prima frase occupa", "quasi una riga intera."]
        );
        assert_eq!(cues[1].lines, vec!["Poi ne arriva un'altra."]);
        assert_eq!(cues.len(), 2);
    }

    #[test]
    fn speaker_labels_open_each_turn() {
        let mut f = file(vec![
            seg(0, 0, "Ciao a tutti.", 300),
            seg(1, 2_000, "Iniziamo subito.", 300),
            seg(2, 4_000, "Va bene.", 300),
            seg(3, 6_000, "Chi parla?", 300),
        ]);
        f.speakers = vec![speaker("meet:anna", "Anna"), speaker("voice:2", "Voice  2")];
        f.segments[0].speaker_id = Some("meet:anna".into());
        f.segments[1].speaker_id = Some("meet:anna".into());
        f.segments[2].speaker_id = Some("voice:2".into());
        f.segments[3].speaker_id = Some("ghost".into());
        let lines: Vec<String> = build_cues(&f)
            .into_iter()
            .map(|c| c.lines.join("|"))
            .collect();
        assert_eq!(
            lines,
            vec![
                "Anna: Ciao a tutti.",
                "Iniziamo subito.", // same turn
                "Voice 2: Va bene.",
                "Chi parla?", // unknown speaker: no label
            ]
        );
    }

    #[test]
    fn without_speakers_there_is_no_prefix() {
        let mut f = file(vec![seg(0, 0, "Ciao.", 300)]);
        f.segments[0].speaker_id = Some("you".into());
        assert_eq!(build_cues(&f)[0].lines, vec!["Ciao."]);
    }

    #[test]
    fn cleaned_text_is_spread_over_the_raw_word_timings() {
        // Raw: 4 words over 0–4 s; cleaned drops the filler "ehm".
        let mut s = seg(0, 0, "ehm allora ci vediamo", 1_000);
        s.text = "Allora, ci vediamo.".into();
        let words = timed_words(&s);
        let got: Vec<(&str, u64, u64)> = words
            .iter()
            .map(|t| (t.w.as_str(), t.start, t.end))
            .collect();
        // Proportional by characters: monotonic, inside the raw span.
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].0, "Allora,");
        assert_eq!(got[0].1, 0);
        assert!(got.windows(2).all(|p| p[0].2 <= p[1].1), "{got:?}");
        assert_eq!(got[2].2, 3_950);
        let cues = build_cues(&file(vec![s]));
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].lines, vec!["Allora, ci vediamo."]);
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (0, 3_950));
    }

    #[test]
    fn same_word_count_keeps_the_measured_timings() {
        let mut s = seg(0, 500, "ciao anna", 1_000);
        s.text = "Ciao Anna!".into();
        let got: Vec<(String, u64, u64)> = timed_words(&s)
            .into_iter()
            .map(|t| (t.w, t.start, t.end))
            .collect();
        assert_eq!(
            got,
            vec![("Ciao".into(), 500, 1_450), ("Anna!".into(), 1_500, 2_450)]
        );
    }

    #[test]
    fn segments_without_or_with_estimated_timings_still_work() {
        let none = Segment {
            id: 0,
            start_ms: 10_000,
            end_ms: 12_000,
            text: "Senza tempi.".into(),
            ..Default::default()
        };
        let mut estimated = seg(1, 20_000, "tempi stimati qui", 500);
        estimated.words_estimated = true;
        let cues = build_cues(&file(vec![none, estimated]));
        assert_well_formed(&cues);
        assert_eq!(cues[0].lines, vec!["Senza tempi."]);
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (10_000, 12_000));
        assert_eq!(cues[1].lines, vec!["tempi stimati qui"]);
        assert_eq!(cues[1].start_ms, 20_000);
    }

    #[test]
    fn empty_and_failed_segments_are_skipped() {
        let mut failed = seg(1, 2_000, "", 300);
        failed.stt_error = Some("model crashed".into());
        failed.end_ms = 5_000;
        let mut failed_with_text = seg(2, 6_000, "testo vecchio", 300);
        failed_with_text.stt_error = Some("boom".into());
        let blank = seg(3, 8_000, "   ", 300);
        let cues = build_cues(&file(vec![
            seg(0, 0, "Uno.", 300),
            failed,
            failed_with_text,
            blank,
            seg(4, 9_000, "Due.", 300),
        ]));
        let lines: Vec<&str> = cues.iter().map(|c| c.lines[0].as_str()).collect();
        assert_eq!(lines, vec!["Uno.", "Due."]);
        assert_eq!(to_srt(&[]), "");
        assert_eq!(to_vtt(&[]), "WEBVTT\n\n");
        assert!(build_cues(&SegmentsFile::default()).is_empty());
    }

    #[test]
    fn overlapping_speakers_never_produce_overlapping_cues() {
        let mut f = file(vec![
            seg(0, 0, "Io parlo per un bel po' di tempo qui", 400), // 0–3.55 s
            seg(1, 1_000, "E io ti interrompo", 300),               // 1–2.15 s
            seg(2, 1_200, "Anche io", 300),                         // 1.2–1.75 s
            seg(3, 1_210, "Pure", 300),
        ]);
        f.speakers = vec![speaker("a", "A"), speaker("b", "B")];
        for (i, s) in f.segments.iter_mut().enumerate() {
            s.speaker_id = Some(if i % 2 == 0 { "a" } else { "b" }.into());
        }
        let cues = build_cues(&f);
        assert_well_formed(&cues);
        assert_eq!(cues.len(), 4);
        // The first is cut where the second starts.
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (0, 1_000));
        let srt = to_srt(&cues);
        assert!(
            srt.starts_with("1\n00:00:00,000 --> 00:00:01,000\nA: Io parlo"),
            "{srt}"
        );
    }

    #[test]
    fn short_cues_are_held_into_the_silence_after_them() {
        let cues = build_cues(&file(vec![
            seg(0, 0, "Sì.", 300),   // 0–250 ms
            seg(1, 600, "No.", 300), // next starts at 600
            seg(2, 5_000, "Ok.", 300),
        ]));
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (0, 600));
        assert_eq!((cues[1].start_ms, cues[1].end_ms), (600, 1_600));
        assert_eq!((cues[2].start_ms, cues[2].end_ms), (5_000, 6_000));
    }

    #[test]
    fn unicode_is_counted_in_characters_not_bytes() {
        // 42 characters, many more bytes.
        let row = "àèéìòù àèéìòù àèéìòù àèéìòù àèéìòù àèéìòù";
        assert_eq!(chars(row), 41);
        let cues = build_cues(&file(vec![seg(0, 0, row, 200)]));
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].lines, vec![row]);
        let text = "perché così è già più facile: città, università, caffè e però ancora";
        for c in build_cues(&file(vec![seg(0, 0, text, 200)])) {
            for l in &c.lines {
                assert!(chars(l) <= MAX_LINE_CHARS, "{l}");
            }
        }
    }

    #[test]
    fn markup_and_arrows_are_made_safe() {
        let cues = build_cues(&file(vec![seg(0, 0, "a --> b <i> & c", 300)]));
        assert_eq!(cues[0].lines, vec!["a -> b <i> & c"]);
        assert!(to_vtt(&cues).contains("a -&gt; b &lt;i&gt; &amp; c"));
        assert!(!to_srt(&cues).contains("a -->"));
    }

    #[test]
    fn a_word_longer_than_a_row_gets_a_row_of_its_own() {
        let url = "https://example.com/a/very/long/path/that/never/fits/in/a/row";
        let cues = build_cues(&file(vec![seg(0, 0, &format!("vedi {url} ok"), 300)]));
        assert_well_formed(&cues);
        let rows: Vec<&String> = cues.iter().flat_map(|c| &c.lines).collect();
        assert!(rows.iter().any(|r| r.as_str() == url), "{rows:?}");
    }

    #[test]
    fn out_of_order_or_out_of_range_word_timings_are_tamed() {
        let mut s = seg(0, 1_000, "a b c", 300);
        s.words = vec![
            word("a", 1_500, 1_200),
            word("b", 900, 5_000),
            word("c", 1_100, 1_300),
        ];
        s.end_ms = 2_000;
        let cues = build_cues(&file(vec![s]));
        assert_well_formed(&cues);
        // Words clamped into the segment (1.5–2 s), then held for the
        // minimum display time.
        assert_eq!(cues.len(), 1);
        assert_eq!((cues[0].start_ms, cues[0].end_ms), (1_500, 2_500));
    }
}
