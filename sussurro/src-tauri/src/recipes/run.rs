//! One recipe run on an archive item: the privacy refusal, the input (the
//! segments, or the markdown when it was edited outside Sussurro), the
//! map-reduce engine, and — for a companion-document recipe — the file
//! next to the transcript with its provenance frontmatter.
//!
//! Also the registry of runs in flight (one per item), for progress and
//! cancel from the UI.

use super::chunk::{has_speakers, lines_from_body, lines_from_segments, InputLine};
use super::engine::{self, ChatModel, Progress};
use super::prompt::Context;
use super::{companion_file_name, Recipe, RecipeTarget};
use crate::archive::companion::{write_companion, CompanionMeta};
use crate::archive::store::TRANSCRIPT_FILE;
use crate::llm::LlmProfile;
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Per-step timeout: a long chunk on a laptop CPU can take minutes.
pub const STEP_TIMEOUT_SECS: u64 = 600;

/// What a finished run produced.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunOutput {
    /// Companion document written (file name in the item folder).
    pub file: Option<String>,
    /// The result, for an answer recipe (nothing is written).
    pub answer: Option<String>,
}

/// Refuse profiles that would send the transcript off this machine. The
/// per-run confirmation that makes them usable is #122; until then nothing
/// leaves the machine from a recipe.
pub fn check_profile(profile: &LlmProfile) -> Result<()> {
    if profile.external {
        bail!(
            "“{}” is an external profile: the transcript would leave this machine. Recipes on external \
             profiles need a confirmation for each run, which arrives in a later update — pick a local \
             profile for now.",
            profile.name
        );
    }
    if profile.model.trim().is_empty() {
        bail!("the profile “{}” has no model — choose one in Recipes → LLM profiles", profile.name);
    }
    Ok(())
}

/// A profile as a [`ChatModel`]: the real HTTP client, with the recipe's
/// timeout and (on Ollama) the planned context window.
pub struct ProfileModel {
    pub profile: LlmProfile,
}

impl ChatModel for ProfileModel {
    fn chat(&self, messages: &[serde_json::Value]) -> Result<String> {
        crate::cleanup::ollama::chat_with(
            &self.profile,
            messages,
            &crate::cleanup::ollama::ChatOptions {
                timeout_secs: STEP_TIMEOUT_SECS,
                num_ctx: Some(self.profile.effective_context_tokens()),
                temperature: 0.2,
            },
        )
    }
}

/// The item as recipe input: from its segments while `transcript.md` is the
/// app's own rendering, else from the markdown body (the markdown wins).
pub fn item_input(item: &crate::archive::Item) -> Vec<InputLine> {
    if !item.edited_externally && !item.segments.segments.is_empty() {
        lines_from_segments(&item.meta, &item.segments)
    } else {
        lines_from_body(&item.body)
    }
}

/// Run `recipe` on item `id` with `model` (the chat side of `profile`).
/// `now` stamps the document (RFC 3339). Nothing is sent when the profile
/// is refused or the item is still being recorded.
#[allow(clippy::too_many_arguments)]
pub fn run_on_item(
    archive: &Path,
    id: &str,
    recipe: &Recipe,
    profile: &LlmProfile,
    model: &dyn ChatModel,
    now: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<RunOutput> {
    check_profile(profile)?;
    let item = crate::archive::read_item(archive, id)?;
    if item.recording {
        bail!("'{id}' is still being recorded — run recipes when the session ends");
    }
    let input = item_input(&item);
    let ctx = Context::from_meta(&item.meta, has_speakers(&input));
    let lines: Vec<String> = input.iter().map(InputLine::format).collect();
    let out = engine::run(
        model,
        recipe,
        &ctx,
        &lines,
        profile.effective_context_tokens(),
        cancel,
        progress,
    )?;
    match recipe.target {
        RecipeTarget::Answer => Ok(RunOutput { file: None, answer: Some(out) }),
        RecipeTarget::CompanionDocument => {
            let meta = CompanionMeta {
                title: format!("{} — {}", item.meta.title.trim(), recipe.name.trim()),
                generated_by: format!("{} / {} / {}", recipe.name.trim(), profile.name.trim(), profile.model.trim()),
                recipe: recipe.id.clone(),
                profile: profile.name.trim().to_string(),
                model: profile.model.trim().to_string(),
                external: profile.external,
                date: now.to_string(),
                transcript: TRANSCRIPT_FILE.to_string(),
                extra: Default::default(),
            };
            let file = write_companion(archive, id, &companion_file_name(recipe), &meta, &out)?;
            Ok(RunOutput { file: Some(file), answer: None })
        }
    }
}

/// A run in flight, as the UI sees it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunStatus {
    pub item_id: String,
    pub recipe_id: String,
    pub recipe_name: String,
    pub progress: Option<Progress>,
}

struct Running {
    status: RunStatus,
    cancel: Arc<AtomicBool>,
}

/// Recipe runs in flight, one per item at most.
#[derive(Default)]
pub struct Runs {
    running: Mutex<HashMap<String, Running>>,
}

impl Runs {
    /// Register a run on `item_id`; refused while another one runs there.
    pub fn begin(&self, item_id: &str, recipe: &Recipe) -> Result<Arc<AtomicBool>> {
        let mut map = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = map.get(item_id) {
            bail!("“{}” is already running on this item — wait for it or cancel it", r.status.recipe_name);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        map.insert(
            item_id.to_string(),
            Running {
                status: RunStatus {
                    item_id: item_id.to_string(),
                    recipe_id: recipe.id.clone(),
                    recipe_name: recipe.name.clone(),
                    progress: None,
                },
                cancel: cancel.clone(),
            },
        );
        Ok(cancel)
    }

    pub fn set_progress(&self, item_id: &str, p: Progress) {
        if let Some(r) = self.running.lock().unwrap_or_else(|e| e.into_inner()).get_mut(item_id) {
            r.status.progress = Some(p);
        }
    }

    pub fn end(&self, item_id: &str) {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).remove(item_id);
    }

    /// Ask the run on `item_id` to stop; false when none runs there.
    pub fn cancel(&self, item_id: &str) -> bool {
        match self.running.lock().unwrap_or_else(|e| e.into_inner()).get(item_id) {
            Some(r) => {
                r.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    pub fn list(&self) -> Vec<RunStatus> {
        let mut v: Vec<RunStatus> = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|r| r.status.clone())
            .collect();
        v.sort_by(|a, b| a.item_id.cmp(&b.item_id));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::super::engine::tests::FakeModel;
    use super::super::{builtin_recipes, Recipe};
    use super::*;
    use crate::archive::companion::{list_companions, read_companion};
    use crate::archive::{create_item, DocSpeaker, ItemMeta, ItemType, Segment, SegmentsFile};
    use crate::settings::CleanupApi;

    const NOW: &str = "2026-09-24T12:00:00+02:00";

    fn meeting(archive: &Path) -> String {
        let meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: "Weekly sync".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            language: "it".into(),
            ..Default::default()
        };
        let segs = SegmentsFile {
            speakers: vec![DocSpeaker { id: "meet:anna".into(), label: "Anna".into(), ..Default::default() }],
            segments: vec![
                Segment {
                    id: 0,
                    start_ms: 61_000,
                    end_ms: 65_000,
                    speaker_id: Some("meet:anna".into()),
                    text: "Mando il file venerdì.".into(),
                    ..Default::default()
                },
                Segment { id: 1, start_ms: 70_000, end_ms: 72_000, text: "Ok.".into(), ..Default::default() },
            ],
            ..Default::default()
        };
        create_item(archive, &meta, &segs).unwrap()
    }

    fn local() -> LlmProfile {
        LlmProfile::new("local", "Local", CleanupApi::Ollama, "http://localhost:11434", "", "llama3.2:3b")
    }

    fn recipe(i: usize) -> Recipe {
        builtin_recipes().remove(i)
    }

    #[test]
    fn formatted_document_is_written_with_provenance() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let model = FakeModel::new("**tl;dr:** Anna manda il file.\n\n# Weekly sync\n");
        let out = run_on_item(&archive, &id, &recipe(0), &local(), &model, NOW, &AtomicBool::new(false), &mut |_| {})
            .unwrap();
        assert_eq!(out, RunOutput { file: Some("document.md".into()), answer: None });

        // Speaker-aware input: timestamps and labels, header, Italian.
        let calls = model.calls.borrow();
        let system = calls[0][0]["content"].as_str().unwrap();
        assert!(system.contains("Write in Italian."));
        assert!(system.contains("speaker's name"));
        let user = model.user(0);
        assert!(user.contains("[00:01:01] Anna: Mando il file venerdì.\n[00:01:10] Ok."), "{user}");
        assert!(user.contains("Title: Weekly sync\nKind: meeting"));

        let raw = std::fs::read_to_string(archive.join(&id).join("document.md")).unwrap();
        assert!(raw.contains("generated_by: Formatted document / Local / llama3.2:3b\n"), "{raw}");
        assert!(raw.contains("transcript: transcript.md\n"));
        assert!(raw.contains("[the transcript](transcript.md)"));
        let doc = read_companion(&archive, &id, "document.md").unwrap();
        assert_eq!(doc.meta.recipe, "formatted-document");
        assert_eq!(doc.meta.date, NOW);
        assert!(!doc.meta.external && !doc.edited_externally);
        assert!(doc.body.contains("# Weekly sync"));
    }

    #[test]
    fn other_recipes_write_their_slug_file() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let model = FakeModel::new("- [ ] Mandare il file — Anna, venerdì");
        run_on_item(&archive, &id, &recipe(2), &local(), &model, NOW, &AtomicBool::new(false), &mut |_| {}).unwrap();
        let files: Vec<_> = list_companions(&archive, &id).unwrap().into_iter().map(|d| d.file).collect();
        assert_eq!(files, ["action-items.md"]);
    }

    #[test]
    fn answer_recipes_return_text_and_write_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let ask = Recipe {
            id: "q".into(),
            name: "Q".into(),
            prompt: "Who sends the file?".into(),
            target: RecipeTarget::Answer,
            builtin: false,
        };
        let model = FakeModel::new("Anna.");
        let out = run_on_item(&archive, &id, &ask, &local(), &model, NOW, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(out, RunOutput { file: None, answer: Some("Anna.".into()) });
        assert!(list_companions(&archive, &id).unwrap().is_empty());
    }

    #[test]
    fn external_profiles_are_refused_before_anything_is_sent() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let work = LlmProfile::new("work", "Work", CleanupApi::Openai, "https://api.example.com/v1", "k", "gpt");
        assert!(work.external);
        let model = FakeModel::new("never");
        let err = run_on_item(&archive, &id, &recipe(1), &work, &model, NOW, &AtomicBool::new(false), &mut |_| {})
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("external profile") && msg.contains("confirmation"), "{msg}");
        assert!(model.calls.borrow().is_empty(), "nothing may reach the model");
        assert!(list_companions(&archive, &id).unwrap().is_empty());

        // A local URL marked external by hand is refused too.
        let mut lan = local();
        lan.external = true;
        assert!(check_profile(&lan).is_err());
        let mut no_model = local();
        no_model.model = " ".into();
        assert!(check_profile(&no_model).is_err());
    }

    #[test]
    fn an_item_edited_outside_uses_its_markdown() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let path = archive.join(&id).join("transcript.md");
        let doc = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, doc.replace("Mando il file venerdì.", "Mando il file lunedì.")).unwrap();
        let model = FakeModel::new("ok");
        run_on_item(&archive, &id, &recipe(1), &local(), &model, NOW, &AtomicBool::new(false), &mut |_| {}).unwrap();
        let user = model.user(0);
        assert!(user.contains("lunedì") && !user.contains("venerdì"), "{user}");
        assert!(!user.contains("# Weekly sync"), "the title heading is not repeated");
    }

    #[test]
    fn a_recording_item_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let mut meta = ItemMeta { title: "Live".into(), date: NOW.into(), ..Default::default() };
        meta.set_session_state(Some(crate::archive::SessionState::Recording));
        let id = create_item(&archive, &meta, &SegmentsFile::default()).unwrap();
        let model = FakeModel::new("x");
        assert!(run_on_item(&archive, &id, &recipe(1), &local(), &model, NOW, &AtomicBool::new(false), &mut |_| {})
            .is_err());
        assert!(model.calls.borrow().is_empty());
    }

    #[test]
    fn runs_registry_one_per_item_with_cancel() {
        let runs = Runs::default();
        let flag = runs.begin("a", &recipe(0)).unwrap();
        assert!(runs.begin("a", &recipe(1)).is_err());
        assert!(runs.begin("b", &recipe(1)).is_ok());
        runs.set_progress("a", Progress { phase: engine::Phase::Map, done: 1, total: 3 });
        let list = runs.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].progress.unwrap().done, 1);
        assert!(runs.cancel("a"));
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
        runs.end("a");
        assert!(!runs.cancel("a"));
        assert!(runs.begin("a", &recipe(0)).is_ok());
    }

    /// Needs a running Ollama with llama3.2:3b pulled. Runs every built-in
    /// recipe on a short Italian meeting and prints the documents. Run:
    /// cargo test live_recipes_on_ollama -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_recipes_on_ollama() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let profile = local();
        let model = ProfileModel { profile: profile.clone() };
        for r in builtin_recipes() {
            let out = run_on_item(&archive, &id, &r, &profile, &model, NOW, &AtomicBool::new(false), &mut |p| {
                println!("{}: {p:?}", r.name)
            })
            .unwrap();
            let doc = read_companion(&archive, &id, out.file.as_deref().unwrap()).unwrap();
            println!("---- {} ----\n{}", doc.file, doc.body);
            assert!(!doc.body.trim().is_empty());
        }
        let doc = read_companion(&archive, &id, "document.md").unwrap();
        assert!(doc.body.to_lowercase().contains("tl;dr"), "{}", doc.body);
    }
}
