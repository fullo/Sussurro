//! Map-reduce orchestration of one recipe run over a chat model.
//!
//! 1. If the whole transcript fits the model's window (see
//!    [`chunk::input_budget_chars`]), one call does it.
//! 2. Otherwise the transcript lines are packed into chunks that fit
//!    (**map**): each chunk yields notes for the task. Chunks follow speaker
//!    turns when the transcript names speakers (#143, see
//!    [`super::chunk::chunk_turns`]).
//! 3. **Reduce**: when all notes fit, one last call turns them into the
//!    result; when they don't, consecutive notes are merged in groups that
//!    fit, level by level, until they do.
//!
//! Cancellation is checked before every call; a call in flight finishes
//! (or times out) first. The model is behind [`ChatModel`] so the
//! orchestration is tested with a fake.

use super::chunk::{chunk_turns, input_budget_chars, InputLine};
use super::prompt::{
    format_notes, map_messages, merge_messages, messages_chars, reduce_messages, single_messages,
    Context,
};
use super::Recipe;
use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

/// Merge levels before the notes are cut to fit (each level at least
/// halves the number of notes, so real transcripts never get here).
const MAX_MERGE_LEVELS: usize = 6;

/// A chat completion endpoint: a profile in the app, a fake in tests.
pub trait ChatModel {
    fn chat(&self, messages: &[Value]) -> Result<String>;
}

/// The run was cancelled by the user (distinct so callers can tell).
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Which step a run is in.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The whole transcript in one call.
    Single,
    /// Notes on each chunk.
    Map,
    /// Merging notes (a long transcript's notes that still don't fit).
    Merge,
    /// The final result from the notes.
    Reduce,
}

/// Progress of a run: `done` of `total` calls of the current phase.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
}

/// Remove what small models wrap around a markdown answer: reasoning
/// blocks (`<think>…</think>`, qwen3 / deepseek-r1) and a code fence
/// around the whole reply.
pub fn clean_output(raw: &str) -> String {
    let mut s = raw.to_string();
    while let Some(start) = s.find("<think>") {
        match s[start..].find("</think>") {
            Some(end) => s.replace_range(start..start + end + "</think>".len(), ""),
            // Unclosed: the model ran out of tokens while thinking.
            None => s.truncate(start),
        }
    }
    let t = s.trim();
    if let Some(inner) = t.strip_prefix("```") {
        if let Some(inner) = inner.strip_suffix("```") {
            // Drop the info string (```markdown) on the opening line.
            let inner = inner.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
            return inner.trim().to_string();
        }
    }
    t.to_string()
}

fn check(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        bail!(Cancelled);
    }
    Ok(())
}

/// Cut `s` to at most `max` characters (last resort when notes won't fit).
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Cut notes to at most `max` characters (plus an ellipsis) on whole
/// lines, so no bullet loses the speaker it starts with (#143); only a
/// first line longer than `max` is cut inside (it keeps its start, speaker
/// included).
fn truncate_notes(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut len = 0;
    for line in s.lines() {
        let l = line.chars().count();
        let sep = usize::from(!out.is_empty());
        if len + sep + l > max {
            break;
        }
        if sep == 1 {
            out.push('\n');
        }
        out.push_str(line);
        len += sep + l;
    }
    if out.is_empty() {
        return truncate_chars(s, max);
    }
    out.push('…');
    out
}

/// Run `recipe` on the formatted transcript `lines` with a model whose
/// window is `context_tokens`. Returns the cleaned markdown. (Plain text
/// lines; [`run_input`] keeps speaker turns together.)
pub fn run(
    model: &dyn ChatModel,
    recipe: &Recipe,
    ctx: &Context,
    lines: &[String],
    context_tokens: u32,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<String> {
    let input: Vec<InputLine> = lines
        .iter()
        .map(|l| InputLine {
            text: l.clone(),
            ..Default::default()
        })
        .collect();
    run_input(model, recipe, ctx, &input, context_tokens, cancel, progress)
}

/// Run `recipe` on the transcript's input lines: as [`run`], with map
/// chunks cut on speaker turns and every piece keeping its speaker (#143).
pub fn run_input(
    model: &dyn ChatModel,
    recipe: &Recipe,
    ctx: &Context,
    input: &[InputLine],
    context_tokens: u32,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<String> {
    let lines: Vec<String> = input.iter().map(InputLine::format).collect();
    if recipe.prompt.trim().is_empty() {
        bail!(
            "the recipe “{}” has no prompt — write one in Recipes",
            recipe.name
        );
    }
    if lines.is_empty() {
        bail!("this item has no text to run a recipe on");
    }
    let call = |msgs: &[Value]| -> Result<String> { Ok(clean_output(&model.chat(msgs)?)) };

    // 1. Everything at once.
    let transcript = lines.join("\n");
    let single_budget = input_budget_chars(
        context_tokens,
        messages_chars(&single_messages(recipe, ctx, "")),
    );
    if transcript.chars().count() <= single_budget {
        check(cancel)?;
        progress(Progress {
            phase: Phase::Single,
            done: 0,
            total: 1,
        });
        let out = call(&single_messages(recipe, ctx, &transcript))?;
        progress(Progress {
            phase: Phase::Single,
            done: 1,
            total: 1,
        });
        return non_empty(out);
    }

    // 2. Map.
    let map_budget = input_budget_chars(
        context_tokens,
        messages_chars(&map_messages(recipe, ctx, "", 99, 99)),
    );
    let chunks = chunk_turns(input, map_budget);
    let total = chunks.len();
    let mut notes = Vec::with_capacity(total);
    for (i, chunk) in chunks.iter().enumerate() {
        check(cancel)?;
        progress(Progress {
            phase: Phase::Map,
            done: i,
            total,
        });
        let n = call(&map_messages(recipe, ctx, chunk, i + 1, total))?;
        notes.push(if n.is_empty() {
            "- (nothing relevant)".to_string()
        } else {
            n
        });
    }
    progress(Progress {
        phase: Phase::Map,
        done: total,
        total,
    });

    // 3. Reduce, merging level by level while the notes don't fit.
    let reduce_budget = input_budget_chars(
        context_tokens,
        messages_chars(&reduce_messages(recipe, ctx, "")),
    );
    let merge_budget = input_budget_chars(
        context_tokens,
        messages_chars(&merge_messages(recipe, ctx, "")),
    );
    let mut level = 0;
    loop {
        let all = format_notes(&notes, 1);
        if all.chars().count() <= reduce_budget {
            check(cancel)?;
            progress(Progress {
                phase: Phase::Reduce,
                done: 0,
                total: 1,
            });
            let out = call(&reduce_messages(recipe, ctx, &all))?;
            progress(Progress {
                phase: Phase::Reduce,
                done: 1,
                total: 1,
            });
            return non_empty(out);
        }
        level += 1;
        if level > MAX_MERGE_LEVELS || notes.len() == 1 {
            // Notes that no merge can shrink enough: cut them to fit.
            let share = (reduce_budget / notes.len()).saturating_sub(16).max(1);
            notes = notes.iter().map(|n| truncate_notes(n, share)).collect();
            continue;
        }
        // Group consecutive notes that fit one merge call together. A group
        // of one would not shrink anything, so a note over half the budget
        // is cut first to pair with its neighbour.
        let half = merge_budget / 2;
        let sized: Vec<String> = notes
            .iter()
            .map(|n| {
                if n.chars().count() > half {
                    truncate_notes(n, half.saturating_sub(32).max(1))
                } else {
                    n.clone()
                }
            })
            .collect();
        let groups = group_notes(&sized, merge_budget);
        let total = groups.len();
        let mut merged = Vec::with_capacity(total);
        let mut first = 1;
        for (i, g) in groups.iter().enumerate() {
            check(cancel)?;
            progress(Progress {
                phase: Phase::Merge,
                done: i,
                total,
            });
            if g.len() == 1 {
                merged.push(g[0].clone());
            } else {
                let n = call(&merge_messages(recipe, ctx, &format_notes(g, first)))?;
                merged.push(if n.is_empty() {
                    format_notes(g, first)
                } else {
                    n
                });
            }
            first += g.len();
        }
        progress(Progress {
            phase: Phase::Merge,
            done: total,
            total,
        });
        notes = merged;
    }
}

/// Consecutive notes packed into groups whose formatted size fits `budget`.
fn group_notes(notes: &[String], budget: usize) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for n in notes {
        let mut candidate = current.clone();
        candidate.push(n.clone());
        if !current.is_empty() && format_notes(&candidate, 1).chars().count() > budget {
            groups.push(std::mem::take(&mut current));
            current.push(n.clone());
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn non_empty(out: String) -> Result<String> {
    if out.trim().is_empty() {
        bail!("the model returned an empty result — try again or pick another profile");
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Reply to a merge call, from its user message.
    pub(crate) type MergeFn = Box<dyn Fn(&str) -> String>;

    /// A fake model: records every call and answers by step kind.
    pub(crate) struct FakeModel {
        pub calls: RefCell<Vec<Vec<Value>>>,
        /// Reply for a map call on part N (N = 1-based).
        pub map_reply: Box<dyn Fn(usize) -> String>,
        pub merge_reply: String,
        /// Reply to a merge call from its user message, when set (else
        /// `merge_reply`).
        pub merge_with: Option<MergeFn>,
        pub final_reply: String,
        /// Set the flag after this many calls.
        pub cancel_after: Option<(usize, std::sync::Arc<AtomicBool>)>,
    }

    impl FakeModel {
        pub(crate) fn new(final_reply: &str) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                map_reply: Box::new(|i| format!("- note {i}")),
                merge_reply: "- merged".into(),
                merge_with: None,
                final_reply: final_reply.into(),
                cancel_after: None,
            }
        }

        pub(crate) fn user(&self, i: usize) -> String {
            self.calls.borrow()[i][1]["content"]
                .as_str()
                .unwrap()
                .to_string()
        }
    }

    impl ChatModel for FakeModel {
        fn chat(&self, messages: &[Value]) -> Result<String> {
            self.calls.borrow_mut().push(messages.to_vec());
            if let Some((n, flag)) = &self.cancel_after {
                if self.calls.borrow().len() >= *n {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            let u = messages[1]["content"].as_str().unwrap();
            if let Some(rest) = u.strip_prefix("This is part ") {
                let part: usize = rest.split(' ').next().unwrap().parse().unwrap();
                return Ok((self.map_reply)(part));
            }
            if u.contains("Merge these notes") {
                if let Some(f) = &self.merge_with {
                    return Ok(f(u));
                }
                return Ok(self.merge_reply.clone());
            }
            Ok(self.final_reply.clone())
        }
    }

    fn recipe() -> Recipe {
        super::super::builtin_recipes().remove(1) // Summary
    }

    fn lines(n: usize, len: usize) -> Vec<String> {
        (0..n)
            .map(|i| format!("[00:00:{:02}] {}", i % 60, "x".repeat(len)))
            .collect()
    }

    fn run_fake(
        model: &FakeModel,
        lines: &[String],
        ctx_tokens: u32,
    ) -> (Result<String>, Vec<Progress>) {
        let mut seen = Vec::new();
        let cancel = model
            .cancel_after
            .as_ref()
            .map(|(_, f)| f.clone())
            .unwrap_or_else(|| std::sync::Arc::new(AtomicBool::new(false)));
        let r = run(
            model,
            &recipe(),
            &Context::default(),
            lines,
            ctx_tokens,
            &cancel,
            &mut |p| seen.push(p),
        );
        (r, seen)
    }

    #[test]
    fn short_input_is_one_call() {
        let m = FakeModel::new("# Summary\nok");
        let (r, progress) = run_fake(&m, &lines(5, 20), 4096);
        assert_eq!(r.unwrap(), "# Summary\nok");
        assert_eq!(m.calls.borrow().len(), 1);
        assert!(m.user(0).starts_with("Task:\nSummarise"));
        assert_eq!(
            progress.last().unwrap(),
            &Progress {
                phase: Phase::Single,
                done: 1,
                total: 1
            }
        );
    }

    #[test]
    fn long_input_maps_every_chunk_then_reduces_once() {
        let m = FakeModel::new("final");
        // ~40 KB of transcript on a 4096-token window: several chunks.
        let input = lines(400, 90);
        let (r, progress) = run_fake(&m, &input, 4096);
        assert_eq!(r.unwrap(), "final");
        let calls = m.calls.borrow().len();
        let maps = (0..calls)
            .filter(|&i| m.user(i).starts_with("This is part "))
            .count();
        assert!(maps > 3, "{maps} map calls");
        assert_eq!(calls, maps + 1, "one reduce after the maps");
        // Every map chunk fits the budget the prompt was sized for.
        let budget = input_budget_chars(4096, 0);
        for i in 0..maps {
            let u = m.user(i);
            assert!(u.starts_with(&format!("This is part {} of {maps}", i + 1)));
            let chunk = u.split("\">\n").nth(1).unwrap();
            assert!(chunk.chars().count() <= budget);
        }
        // The chunks cover the whole transcript, in order.
        let covered: Vec<String> = (0..maps)
            .flat_map(|i| {
                let u = m.user(i);
                let body = u
                    .split("\">\n")
                    .nth(1)
                    .unwrap()
                    .trim_end_matches("\n</transcript>")
                    .to_string();
                body.lines().map(str::to_string).collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(covered, input);
        // The reduce sees every note, numbered.
        let reduce = m.user(calls - 1);
        assert!(
            reduce.contains(&format!("### Part {maps}\n- note {maps}")),
            "{reduce}"
        );
        assert!(progress.contains(&Progress {
            phase: Phase::Map,
            done: maps,
            total: maps
        }));
        assert_eq!(progress.last().unwrap().phase, Phase::Reduce);
    }

    #[test]
    fn a_bigger_window_means_fewer_chunks() {
        let small = FakeModel::new("x");
        run_fake(&small, &lines(400, 90), 4096).0.unwrap();
        let big = FakeModel::new("x");
        run_fake(&big, &lines(400, 90), 32_768).0.unwrap();
        assert!(big.calls.borrow().len() < small.calls.borrow().len());
    }

    #[test]
    fn notes_that_do_not_fit_are_merged_before_the_final_reduce() {
        let mut m = FakeModel::new("final");
        // Verbose notes: each map reply is ~2 KB, so 20+ of them overflow.
        m.map_reply = Box::new(|i| format!("- note {i} {}", "n".repeat(2000)));
        let (r, progress) = run_fake(&m, &lines(400, 90), 4096);
        assert_eq!(r.unwrap(), "final");
        let calls = m.calls.borrow().len();
        let merges = (0..calls)
            .filter(|&i| m.user(i).contains("Merge these notes"))
            .count();
        assert!(merges > 0);
        assert!(progress.iter().any(|p| p.phase == Phase::Merge));
        // Merge inputs fit the window too.
        let budget = input_budget_chars(4096, 0);
        for i in 0..calls {
            let u = m.user(i);
            if let Some(notes) = u.split("<notes>\n").nth(1) {
                assert!(
                    notes.chars().count() <= budget,
                    "call {i}: {}",
                    notes.chars().count()
                );
            }
        }
        assert!(m.user(calls - 1).starts_with("Task:\n"));
    }

    #[test]
    fn cancel_stops_before_the_next_call() {
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let mut m = FakeModel::new("final");
        m.cancel_after = Some((2, flag));
        let (r, _) = run_fake(&m, &lines(400, 90), 4096);
        let err = r.unwrap_err();
        assert!(err.downcast_ref::<Cancelled>().is_some(), "{err:#}");
        assert_eq!(m.calls.borrow().len(), 2);
    }

    #[test]
    fn empty_prompt_input_or_reply_are_errors() {
        let m = FakeModel::new("  ");
        assert!(run_fake(&m, &lines(2, 5), 4096).0.is_err());
        let m = FakeModel::new("x");
        assert!(run_fake(&m, &[], 4096).0.is_err());
        let empty = Recipe {
            name: "E".into(),
            ..Default::default()
        };
        let r = run(
            &m,
            &empty,
            &Context::default(),
            &lines(2, 5),
            4096,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(format!("{:#}", r.unwrap_err()).contains("no prompt"));
        assert!(m.calls.borrow().is_empty());
    }

    #[test]
    fn clean_output_strips_thinking_and_fences() {
        assert_eq!(clean_output("<think>hmm</think>\n# Doc"), "# Doc");
        assert_eq!(clean_output("```markdown\n# Doc\n- a\n```"), "# Doc\n- a");
        assert_eq!(clean_output("```\nx\n```\n"), "x");
        assert_eq!(clean_output("<think>never closed"), "");
        assert_eq!(clean_output("a ```code``` b"), "a ```code``` b");
    }

    #[test]
    fn grouping_keeps_order_and_budget() {
        let notes: Vec<String> = (0..10)
            .map(|i| format!("- {i} {}", "y".repeat(50)))
            .collect();
        let groups = group_notes(&notes, 200);
        assert!(groups.len() > 1);
        assert_eq!(groups.concat(), notes);
        for g in &groups {
            assert!(format_notes(g, 1).chars().count() <= 200 || g.len() == 1);
        }
    }

    // ---- #143: speaker turns and attribution through map-reduce ----

    const SPEAKERS: [&str; 3] = ["Anna Rossi", "Ben", "Voice 1"];

    /// `turns` turns of three lines each, speakers in rotation.
    fn speaker_input(turns: usize) -> Vec<InputLine> {
        (0..turns)
            .flat_map(|t| {
                (0..3).map(move |k| InputLine {
                    start_ms: Some(((t * 3 + k) * 1000) as u64),
                    speaker: Some(SPEAKERS[t % 3].to_string()),
                    text: format!("turn {t} line {k} {}", "x".repeat(70)),
                })
            })
            .collect()
    }

    fn speaker_ctx() -> Context {
        Context {
            speakers: true,
            timestamps: true,
            speaker_names: SPEAKERS.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    /// The speaker a fake map note attributes point `i.k` to.
    fn owner(i: usize, k: usize) -> &'static str {
        SPEAKERS[(i + k) % 3]
    }

    /// `- [..] Speaker: point i.k` bullets of a message, as (point, speaker).
    fn attributed(u: &str) -> Vec<(String, String)> {
        u.lines()
            .filter_map(|l| l.strip_prefix("- [00:00:00] "))
            .filter_map(|l| l.split_once(": point "))
            .map(|(sp, rest)| {
                (
                    rest.split_whitespace()
                        .next()
                        .unwrap()
                        .trim_end_matches('…')
                        .to_string(),
                    sp.to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn map_chunks_start_on_speaker_turns() {
        let m = FakeModel::new("final");
        let input = speaker_input(60);
        let r = run_input(
            &m,
            &recipe(),
            &speaker_ctx(),
            &input,
            4096,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(r.unwrap(), "final");
        let calls = m.calls.borrow().len();
        let maps: Vec<String> = (0..calls)
            .map(|i| m.user(i))
            .filter(|u| u.starts_with("This is part "))
            .collect();
        assert!(maps.len() > 2);
        let mut covered = Vec::new();
        for u in &maps {
            assert!(
                u.contains("speaker by speaker"),
                "map notes are asked per speaker"
            );
            let body = u
                .split("\">\n")
                .nth(1)
                .unwrap()
                .trim_end_matches("\n</transcript>");
            let body: Vec<&str> = body.lines().filter(|l| l.starts_with('[')).collect();
            assert!(
                body[0].contains(" line 0 "),
                "a chunk starts mid-turn: {}",
                body[0]
            );
            covered.extend(body.iter().map(|l| l.to_string()));
        }
        let formatted: Vec<String> = input.iter().map(InputLine::format).collect();
        assert_eq!(covered, formatted, "every line once, in order");
    }

    #[test]
    fn merges_and_the_reduce_keep_each_statement_with_its_speaker() {
        let mut m = FakeModel::new("final");
        // Verbose attributed notes: 25 bullets per part, so merges are needed.
        m.map_reply = Box::new(|i| {
            (0..25)
                .map(|k| {
                    format!(
                        "- [00:00:00] {}: point {i}.{k} {}",
                        owner(i, k),
                        "n".repeat(60)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        });
        // A merge that keeps a few bullets of each part verbatim.
        m.merge_with = Some(Box::new(|u: &str| {
            u.split("### Part ")
                .skip(1)
                .flat_map(|part| {
                    part.lines()
                        .filter(|l| l.starts_with("- ["))
                        .take(3)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        }));
        let input = speaker_input(90);
        let (tx, mut phases) = (&AtomicBool::new(false), Vec::new());
        let r = run_input(&m, &recipe(), &speaker_ctx(), &input, 4096, tx, &mut |p| {
            phases.push(p.phase)
        });
        assert_eq!(r.unwrap(), "final");
        assert!(phases.contains(&Phase::Merge), "the notes needed merging");
        let calls = m.calls.borrow().len();
        let mut checked = 0;
        for i in 0..calls {
            let u = m.user(i);
            if u.starts_with("This is part ") {
                continue;
            }
            if u.contains("Merge these notes") {
                assert!(
                    u.contains("never combine bullets of different speakers"),
                    "{u}"
                );
            } else {
                assert!(u.contains("keep that attribution exactly"), "{u}");
            }
            // Every bullet the step reads is whole-headed and still carries
            // the speaker the map gave it.
            for (point, sp) in attributed(&u) {
                let (i, k) = point.split_once('.').unwrap();
                assert_eq!(
                    sp,
                    owner(i.parse().unwrap(), k.parse().unwrap()),
                    "point {point}"
                );
                checked += 1;
            }
        }
        assert!(checked > 20, "{checked} attributed bullets checked");
        let reduce = m.user(calls - 1);
        assert!(reduce.starts_with("Task:\n") && !attributed(&reduce).is_empty());
    }

    #[test]
    fn notes_are_cut_on_whole_lines() {
        let notes = "- Anna: one\n- Ben: two\n- Voice 1: three";
        assert_eq!(truncate_notes(notes, 100), notes);
        assert_eq!(truncate_notes(notes, 22), "- Anna: one\n- Ben: two…");
        assert_eq!(truncate_notes(notes, 12), "- Anna: one…");
        // A first line longer than the cut: cut inside, start kept.
        assert_eq!(truncate_notes(notes, 5), "- Ann…");
    }
}
