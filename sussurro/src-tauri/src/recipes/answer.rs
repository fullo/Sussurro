//! Answers for the Ask panel (0.8, #121): free questions on the open
//! document, and what an *answer* recipe returned.
//!
//! A free question is a transient [`RecipeTarget::Answer`] recipe whose task
//! is the question: it runs through the same engine (map-reduce on long
//! transcripts) as every other recipe. An answer lives only in memory
//! ([`Answers`]) until the user saves it; saving writes a companion
//! document with provenance frontmatter next to the transcript and never
//! replaces an existing file.

use super::{Recipe, RecipeTarget};
use crate::archive::companion::{write_companion_new, CompanionMeta, KIND_ANSWER, KIND_KEY};
use crate::archive::paths::slugify;
use crate::archive::store::TRANSCRIPT_FILE;
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

/// Id of the transient free-question recipe (reserved: no user recipe gets
/// it, see [`super::normalize_user_recipes`]).
pub const QUESTION_RECIPE_ID: &str = "question";
/// Display name of a free-question run.
pub const QUESTION_RECIPE_NAME: &str = "Question";
/// Longest question accepted, in characters.
pub const MAX_QUESTION_CHARS: usize = 2000;
/// Answers kept in memory for "Save as document" (oldest dropped first).
const MAX_PENDING: usize = 32;
/// Characters of the question kept in a saved answer's title.
const TITLE_QUESTION_CHARS: usize = 80;

/// The question as asked: whitespace runs (newlines included) collapsed to
/// one space, trimmed. Refused when empty or too long.
pub fn normalize_question(question: &str) -> Result<String> {
    let q = question.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.is_empty() {
        bail!("write a question first");
    }
    if q.chars().count() > MAX_QUESTION_CHARS {
        bail!("the question is too long (at most {MAX_QUESTION_CHARS} characters)");
    }
    Ok(q)
}

/// The task of a free-question recipe: the question, fenced, and how to
/// answer it. The engine adds the transcript (or, on long transcripts, the
/// notes of its parts) and the shared system prompt.
pub fn question_prompt(question: &str) -> String {
    format!(
        "Answer this question about the transcript:\n<question>\n{question}\n</question>\n\
         Answer only from what the transcript says; if it does not say, reply in one line that the \
         transcript does not cover it. Be direct: a short paragraph, or a bullet list when the answer \
         has several parts. Say who said what when the transcript names speakers."
    )
}

/// The transient recipe that asks `question` (normalized first).
pub fn question_recipe(question: &str) -> Result<Recipe> {
    let q = normalize_question(question)?;
    Ok(Recipe {
        id: QUESTION_RECIPE_ID.into(),
        name: QUESTION_RECIPE_NAME.into(),
        prompt: question_prompt(&q),
        target: RecipeTarget::Answer,
        builtin: true,
    })
}

/// A finished answer with its provenance, kept until saved or dropped.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PendingAnswer {
    pub item_id: String,
    pub recipe_id: String,
    pub recipe_name: String,
    /// The question, for a free question; `None` for an answer recipe.
    pub question: Option<String>,
    pub profile: String,
    pub model: String,
    pub external: bool,
    /// Server the transcript went to, for an external profile (#122).
    pub host: String,
    /// When it was generated, RFC 3339.
    pub date: String,
    /// The answer (markdown).
    pub text: String,
}

/// `s` cut to `max` characters, with an ellipsis when cut.
fn shorten(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", s[..i].trim_end()),
        None => s.to_string(),
    }
}

/// File a saved answer asks for: `<slug of the question>.md` for a free
/// question, `<slug of the recipe name>.md` for an answer recipe. A slug
/// that would name the transcript or the formatted document gets a prefix.
/// (The writer then picks `-2`, `-3`… when the name is taken.)
pub fn answer_file_name(answer: &PendingAnswer) -> String {
    let (slug, prefix) = match &answer.question {
        Some(q) => (slugify(q), "question"),
        None => (slugify(&answer.recipe_name), "recipe"),
    };
    match slug.as_str() {
        "transcript" | "document" => format!("{prefix}-{slug}.md"),
        _ => format!("{slug}.md"),
    }
}

/// The saved document for `answer` on the item titled `item_title`: file
/// name asked for, provenance frontmatter, body.
pub fn answer_document(item_title: &str, answer: &PendingAnswer) -> (String, CompanionMeta, String) {
    let item_title = item_title.trim();
    let (title, body) = match &answer.question {
        Some(q) => (
            format!("{item_title} — {}", shorten(q, TITLE_QUESTION_CHARS)),
            format!("> **Question:** {q}\n\n{}", answer.text.trim()),
        ),
        None => (format!("{item_title} — {}", answer.recipe_name.trim()), answer.text.trim().to_string()),
    };
    let mut extra = std::collections::BTreeMap::new();
    extra.insert(KIND_KEY.to_string(), serde_json::Value::from(KIND_ANSWER));
    if let Some(q) = &answer.question {
        extra.insert("question".to_string(), serde_json::Value::from(q.as_str()));
    }
    let meta = CompanionMeta {
        title,
        generated_by: format!(
            "{} / {} / {}",
            answer.recipe_name.trim(),
            answer.profile.trim(),
            answer.model.trim()
        ),
        recipe: answer.recipe_id.clone(),
        profile: answer.profile.trim().to_string(),
        model: answer.model.trim().to_string(),
        external: answer.external,
        host: if answer.external { answer.host.trim().to_string() } else { String::new() },
        date: answer.date.clone(),
        transcript: TRANSCRIPT_FILE.to_string(),
        extra,
    };
    (answer_file_name(answer), meta, body)
}

/// Save `answer` next to the transcript of its item. Returns the file name
/// written (never an existing file's).
pub fn save_answer(archive: &Path, answer: &PendingAnswer) -> Result<String> {
    if answer.text.trim().is_empty() {
        bail!("the answer is empty — nothing to save");
    }
    let item = crate::archive::read_item(archive, &answer.item_id)?;
    let (file, meta, body) = answer_document(&item.meta.title, answer);
    write_companion_new(archive, &answer.item_id, &file, &meta, &body)
}

/// Answers not saved yet, in memory only (bounded; oldest dropped first).
#[derive(Default)]
pub struct Answers {
    inner: Mutex<(u64, VecDeque<(u64, PendingAnswer)>)>,
}

impl Answers {
    /// Keep `answer`; returns the id to save or drop it by.
    pub fn put(&self, answer: PendingAnswer) -> u64 {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.0 += 1;
        let id = g.0;
        g.1.push_back((id, answer));
        while g.1.len() > MAX_PENDING {
            g.1.pop_front();
        }
        id
    }

    /// The answer `id` of item `item_id`, if still kept.
    pub fn get(&self, item_id: &str, id: u64) -> Option<PendingAnswer> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.1.iter()
            .find(|(i, a)| *i == id && a.item_id == item_id)
            .map(|(_, a)| a.clone())
    }

    /// Forget answer `id` of item `item_id` (saved or dismissed).
    pub fn remove(&self, item_id: &str, id: u64) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before = g.1.len();
        g.1.retain(|(i, a)| !(*i == id && a.item_id == item_id));
        g.1.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::super::engine::tests::FakeModel;
    use super::super::engine::{Phase, Progress};
    use super::super::prompt::{map_messages, reduce_messages, single_messages, Context};
    use super::super::run::{run_on_item, RunOutput};
    use super::*;
    use crate::archive::companion::{list_companions, parse_companion, read_companion};
    use crate::archive::{create_item, ItemMeta, ItemType, Segment, SegmentsFile};
    use crate::llm::LlmProfile;
    use crate::settings::CleanupApi;
    use std::sync::atomic::AtomicBool;

    const NOW: &str = "2026-09-24T12:00:00+02:00";

    fn user(msgs: &[serde_json::Value]) -> String {
        msgs[1]["content"].as_str().unwrap().to_string()
    }

    #[test]
    fn questions_are_normalized_and_bounded() {
        assert_eq!(normalize_question("  Who sends\n the   file? ").unwrap(), "Who sends the file?");
        assert!(normalize_question(" \n\t ").is_err());
        assert!(normalize_question(&"x".repeat(MAX_QUESTION_CHARS)).is_ok());
        let err = normalize_question(&"x".repeat(MAX_QUESTION_CHARS + 1)).unwrap_err();
        assert!(format!("{err}").contains("too long"));
    }

    #[test]
    fn a_question_is_a_transient_answer_recipe() {
        let r = question_recipe(" Chi manda il file?\n").unwrap();
        assert_eq!(r.id, QUESTION_RECIPE_ID);
        assert_eq!(r.name, "Question");
        assert_eq!(r.target, RecipeTarget::Answer);
        assert!(r.prompt.starts_with("Answer this question about the transcript:\n<question>\nChi manda il file?\n</question>\n"));
        assert!(r.prompt.contains("does not cover it"));
        assert!(question_recipe("").is_err());
    }

    #[test]
    fn question_prompt_assembly_single_map_and_reduce() {
        let r = question_recipe("Who sends the file?").unwrap();
        let ctx = Context { title: "Weekly sync".into(), kind: "meeting".into(), ..Default::default() };
        // Whole transcript: the question is the task, then the fenced transcript.
        let single = user(&single_messages(&r, &ctx, "[00:01:01] Anna: I send it Friday."));
        assert!(single.starts_with("Task:\nAnswer this question about the transcript:\n<question>\nWho sends the file?\n</question>"));
        assert!(single.contains("<transcript>\nTitle: Weekly sync\nKind: meeting\n[00:01:01] Anna: I send it Friday.\n</transcript>"));
        // Long transcript: every map step keeps the question as the task…
        let map = user(&map_messages(&r, &ctx, "chunk", 1, 3));
        assert!(map.contains("<task>\nAnswer this question about the transcript:\n<question>\nWho sends the file?"));
        assert!(map.contains("<transcript part=\"1/3\">"));
        // …and the reduce answers it from the notes.
        let reduce = user(&reduce_messages(&r, &ctx, "### Part 1\n- a"));
        assert!(reduce.starts_with("Task:\nAnswer this question"));
        assert!(reduce.contains("<notes>\nTitle: Weekly sync"));
    }

    fn meeting(archive: &Path, lines: usize) -> String {
        let meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: "Weekly sync".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            language: "en".into(),
            ..Default::default()
        };
        let segs = SegmentsFile {
            segments: (0..lines)
                .map(|i| Segment {
                    id: i as u32,
                    start_ms: i as u64 * 1000,
                    end_ms: i as u64 * 1000 + 900,
                    text: format!("Line {i}: Anna sends the file on Friday. {}", "x".repeat(80)),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        create_item(archive, &meta, &segs).unwrap()
    }

    fn local() -> LlmProfile {
        LlmProfile::new("local", "Local", CleanupApi::Ollama, "http://localhost:11434", "", "llama3.2:3b")
    }

    #[test]
    fn a_question_runs_through_map_reduce_and_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        // Long enough for several chunks on the default 4096-token window.
        let id = meeting(&archive, 400);
        let model = FakeModel::new("Anna, on Friday.");
        let mut phases = Vec::new();
        let out = run_on_item(
            &archive,
            &id,
            &question_recipe("Who sends the file?").unwrap(),
            &local(),
            None,
            &model,
            NOW,
            &AtomicBool::new(false),
            &mut |p: Progress| phases.push(p.phase),
        )
        .unwrap();
        assert_eq!(out, RunOutput { file: None, answer: Some("Anna, on Friday.".into()) });
        assert!(phases.contains(&Phase::Map) && phases.last() == Some(&Phase::Reduce));
        let calls = model.calls.borrow().len();
        assert!(calls > 2);
        for i in 0..calls - 1 {
            assert!(model.user(i).contains("<question>\nWho sends the file?\n</question>"));
        }
        assert!(list_companions(&archive, &id).unwrap().is_empty(), "answers are not persisted");
    }

    fn answer(item_id: &str, question: Option<&str>) -> PendingAnswer {
        PendingAnswer {
            item_id: item_id.into(),
            recipe_id: if question.is_some() { QUESTION_RECIPE_ID.into() } else { "open-questions".into() },
            recipe_name: if question.is_some() { QUESTION_RECIPE_NAME.into() } else { "Open questions".into() },
            question: question.map(str::to_string),
            profile: "Local".into(),
            model: "llama3.2:3b".into(),
            external: false,
            host: String::new(),
            date: NOW.into(),
            text: "Anna, on Friday.\n".into(),
        }
    }

    #[test]
    fn a_saved_external_answer_records_its_host() {
        let mut a = answer("x", Some("Who?"));
        a.external = true;
        a.host = "api.example.com".into();
        let (_, meta, _) = answer_document("T", &a);
        assert!(meta.external);
        assert_eq!(meta.host, "api.example.com");
        // A local answer never carries a host, whatever the field says.
        a.external = false;
        assert_eq!(answer_document("T", &a).1.host, "");
    }

    #[test]
    fn saved_answer_names() {
        assert_eq!(answer_file_name(&answer("x", Some("Who sends the file?"))), "who-sends-the-file.md");
        assert_eq!(answer_file_name(&answer("x", Some("Perché è così?"))), "perche-e-cosi.md");
        assert_eq!(answer_file_name(&answer("x", Some("Document?"))), "question-document.md");
        assert_eq!(answer_file_name(&answer("x", Some("Transcript"))), "question-transcript.md");
        assert_eq!(answer_file_name(&answer("x", Some("???"))), "untitled.md");
        let long = answer_file_name(&answer("x", Some(&"word ".repeat(40))));
        assert!(long.len() <= crate::archive::paths::MAX_SLUG_LEN + 3, "{long}");
        assert_eq!(answer_file_name(&answer("x", None)), "open-questions.md");
    }

    #[test]
    fn saved_answer_carries_provenance() {
        let (file, meta, body) = answer_document(" Weekly sync ", &answer("x", Some("Who sends the file?")));
        assert_eq!(file, "who-sends-the-file.md");
        assert_eq!(meta.title, "Weekly sync — Who sends the file?");
        assert_eq!(meta.generated_by, "Question / Local / llama3.2:3b");
        assert_eq!(meta.recipe, "question");
        assert_eq!((meta.profile.as_str(), meta.model.as_str(), meta.external), ("Local", "llama3.2:3b", false));
        assert_eq!(meta.date, NOW);
        assert_eq!(meta.transcript, "transcript.md");
        assert_eq!(meta.extra[KIND_KEY], "answer");
        assert_eq!(meta.extra["question"], "Who sends the file?");
        assert_eq!(body, "> **Question:** Who sends the file?\n\nAnna, on Friday.");

        let (_, meta, body) = answer_document("Weekly sync", &answer("x", None));
        assert_eq!(meta.title, "Weekly sync — Open questions");
        assert_eq!(meta.generated_by, "Open questions / Local / llama3.2:3b");
        assert!(!meta.extra.contains_key("question"));
        assert_eq!(body, "Anna, on Friday.");

        let long = "why ".repeat(60);
        let (_, meta, _) = answer_document("T", &answer("x", Some(long.trim())));
        assert!(meta.title.ends_with('…') && meta.title.chars().count() <= 4 + TITLE_QUESTION_CHARS + 1);
    }

    #[test]
    fn save_writes_a_new_companion_every_time() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive, 2);
        let a = answer(&id, Some("Who sends the file?"));
        assert_eq!(save_answer(&archive, &a).unwrap(), "who-sends-the-file.md");
        assert_eq!(save_answer(&archive, &a).unwrap(), "who-sends-the-file-2.md");
        let raw = std::fs::read_to_string(archive.join(&id).join("who-sends-the-file.md")).unwrap();
        assert!(raw.contains("generated_by: Question / Local / llama3.2:3b\n"), "{raw}");
        assert!(raw.contains("kind: answer\n"), "{raw}");
        assert!(raw.contains("[the transcript](transcript.md)"));
        let (meta, _) = parse_companion(&raw);
        assert_eq!(meta.extra["question"], "Who sends the file?");
        let doc = read_companion(&archive, &id, "who-sends-the-file.md").unwrap();
        assert!(!doc.edited_externally);
        assert!(doc.body.contains("Anna, on Friday."));

        let mut empty = a.clone();
        empty.text = " ".into();
        assert!(save_answer(&archive, &empty).is_err());
        let mut gone = a;
        gone.item_id = "2026/09/nope".into();
        assert!(save_answer(&archive, &gone).is_err());
    }

    #[test]
    fn pending_answers_are_kept_per_item_and_bounded() {
        let store = Answers::default();
        let a = store.put(answer("a", Some("q")));
        let b = store.put(answer("b", None));
        assert_ne!(a, b);
        assert_eq!(store.get("a", a).unwrap().question.as_deref(), Some("q"));
        assert!(store.get("b", a).is_none(), "an answer is only reachable from its item");
        assert!(store.remove("a", a));
        assert!(!store.remove("a", a));
        assert!(store.get("a", a).is_none());
        for _ in 0..MAX_PENDING {
            store.put(answer("c", None));
        }
        assert!(store.get("b", b).is_none(), "the oldest answer was dropped");
        assert_eq!(store.inner.lock().unwrap().1.len(), MAX_PENDING);
    }
}
