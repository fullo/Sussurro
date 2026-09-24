//! One recipe run on an archive item: the privacy gate, the input (the
//! segments, or the markdown when it was edited outside Sussurro), the
//! map-reduce engine, and — for a companion-document recipe — the file
//! next to the transcript with its provenance frontmatter.
//!
//! Privacy gate (#122): a run on an external profile needs a
//! [`ConsentGrant`] issued for exactly this run (see [`crate::llm::consent`]);
//! without one it is refused before anything is read or sent. A granted
//! run is recorded in the item's external-send log before the first
//! request, and its document carries `external: true` and the host.
//!
//! Also the registry of runs in flight (one per item), for progress and
//! cancel from the UI.

use super::chunk::{has_speakers, lines_from_body, lines_from_segments, InputLine};
use super::engine::{self, ChatModel, Progress};
use super::prompt::Context;
use super::{companion_file_name, Recipe, RecipeTarget};
use crate::archive::companion::{write_companion, CompanionMeta};
use crate::archive::external::{ExternalSend, SendKind};
use crate::archive::store::TRANSCRIPT_FILE;
use crate::llm::consent::{authorize, ConsentGrant, RunTarget};
use crate::llm::LlmProfile;
use anyhow::{bail, Context as _, Result};
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

/// A profile a recipe can run on at all: it names a model.
pub fn check_profile(profile: &LlmProfile) -> Result<()> {
    if profile.model.trim().is_empty() {
        bail!("the profile “{}” has no model — choose one in Recipes → LLM profiles", profile.name);
    }
    Ok(())
}

/// What the confirmation dialog shows before an external run (#122): which
/// document goes to which host, and roughly how much of it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ExternalRunPreview {
    pub item_id: String,
    pub item_title: String,
    pub recipe_id: String,
    pub recipe_name: String,
    /// The question, for a free question.
    pub question: Option<String>,
    pub profile_id: String,
    pub profile_name: String,
    /// Server the text goes to (`api.example.com`).
    pub host: String,
    pub base_url: String,
    pub model: String,
    /// Characters of the transcript that will be sent (plus the task).
    pub chars: usize,
    /// Rough token count (≈ 4 characters per token).
    pub approx_tokens: usize,
    /// The profile is external: the run needs a confirmation.
    pub external: bool,
}

/// Rough token count of `chars` characters of text (≈ 4 per token).
pub fn approx_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

/// The preview of running `recipe` on item `id` with `profile`. Reads the
/// item; sends nothing.
pub fn preview(
    archive: &Path,
    id: &str,
    recipe: &Recipe,
    question: Option<&str>,
    profile: &LlmProfile,
) -> Result<ExternalRunPreview> {
    let item = crate::archive::read_item(archive, id)?;
    let input = item_input(&item);
    let chars = input.iter().map(|l| l.format().chars().count() + 1).sum::<usize>()
        + recipe.prompt.chars().count();
    Ok(ExternalRunPreview {
        item_id: id.to_string(),
        item_title: item.meta.title.trim().to_string(),
        recipe_id: recipe.id.clone(),
        recipe_name: recipe.name.clone(),
        question: question.map(str::to_string),
        profile_id: profile.id.clone(),
        profile_name: profile.name.trim().to_string(),
        host: profile.host_label(),
        base_url: profile.base_url.trim().to_string(),
        model: profile.model.trim().to_string(),
        chars,
        approx_tokens: approx_tokens(chars),
        external: profile.external,
    })
}

/// The external-send log entry for a granted run.
fn send_entry(recipe: &Recipe, profile: &LlmProfile, now: &str) -> ExternalSend {
    let question = recipe.id == super::answer::QUESTION_RECIPE_ID;
    ExternalSend {
        date: now.to_string(),
        host: profile.host(),
        profile: profile.name.trim().to_string(),
        model: profile.model.trim().to_string(),
        kind: if question { SendKind::Question } else { SendKind::Recipe },
        recipe: if question { String::new() } else { recipe.id.clone() },
    }
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
/// is refused, when an external profile comes without a matching
/// `consent` (#122), or when the item is still being recorded.
#[allow(clippy::too_many_arguments)]
pub fn run_on_item(
    archive: &Path,
    id: &str,
    recipe: &Recipe,
    profile: &LlmProfile,
    consent: Option<&ConsentGrant>,
    model: &dyn ChatModel,
    now: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<RunOutput> {
    check_profile(profile)?;
    authorize(profile, &RunTarget::new(id, recipe, profile), consent)?;
    let item = crate::archive::read_item(archive, id)?;
    if item.recording {
        bail!("'{id}' is still being recorded — run recipes when the session ends");
    }
    if profile.external {
        // Before the first request: a run that then fails or is cancelled
        // may already have sent part of the transcript.
        crate::archive::external::record(archive, id, &send_entry(recipe, profile, now))
            .context("could not record the external run, so nothing was sent")?;
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
                host: if profile.external { profile.host() } else { String::new() },
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
    /// The question, for a free question from the Ask panel (#121).
    pub question: Option<String>,
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
        self.begin_with(item_id, recipe, None)
    }

    /// [`Runs::begin`] for a free question (shown in the run's status).
    pub fn begin_with(&self, item_id: &str, recipe: &Recipe, question: Option<&str>) -> Result<Arc<AtomicBool>> {
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
                    question: question.map(str::to_string),
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
        let out = run_on_item(&archive, &id, &recipe(0), &local(), None, &model, NOW, &AtomicBool::new(false), &mut |_| {})
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
        run_on_item(&archive, &id, &recipe(2), &local(), None, &model, NOW, &AtomicBool::new(false), &mut |_| {}).unwrap();
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
        let out = run_on_item(&archive, &id, &ask, &local(), None, &model, NOW, &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(out, RunOutput { file: None, answer: Some("Anna.".into()) });
        assert!(list_companions(&archive, &id).unwrap().is_empty());
    }

    fn work() -> LlmProfile {
        LlmProfile::new("work", "Work", CleanupApi::Openai, "https://api.example.com/v1", "k", "gpt")
    }

    fn run(archive: &Path, id: &str, r: &Recipe, p: &LlmProfile, g: Option<&ConsentGrant>, m: &FakeModel) -> Result<RunOutput> {
        run_on_item(archive, id, r, p, g, m, NOW, &AtomicBool::new(false), &mut |_| {})
    }

    /// #122: without a confirmation an external run is refused before
    /// anything is read, logged or sent.
    #[test]
    fn external_runs_without_consent_are_refused_before_anything_is_sent() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        assert!(work().external);
        let model = FakeModel::new("never");
        let err = run(&archive, &id, &recipe(1), &work(), None, &model).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("external profile") && msg.contains("api.example.com") && msg.contains("Confirm"), "{msg}");
        assert!(model.calls.borrow().is_empty(), "nothing may reach the model");
        assert!(list_companions(&archive, &id).unwrap().is_empty());
        assert!(crate::archive::external::read_log(&archive, &id).unwrap().is_empty(), "nothing was sent");

        // A local URL marked external by hand needs the confirmation too.
        let mut lan = local();
        lan.external = true;
        assert!(run(&archive, &id, &recipe(1), &lan, None, &model).is_err());
        assert!(model.calls.borrow().is_empty());
        let mut no_model = local();
        no_model.model = " ".into();
        assert!(check_profile(&no_model).is_err());
    }

    /// A grant issued for another run (item, recipe, profile, host or model)
    /// doesn't open this one.
    #[test]
    fn a_grant_for_another_run_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let store = crate::llm::consent::ConsentStore::default();
        let for_summary = RunTarget::new(&id, &recipe(1), &work());
        let grant = store.consume(&store.issue(for_summary.clone()), &for_summary).unwrap();
        let model = FakeModel::new("never");
        // Another recipe.
        assert!(run(&archive, &id, &recipe(2), &work(), Some(&grant), &model).is_err());
        // Same recipe, the profile's model changed after confirming.
        let mut changed = work();
        changed.model = "gpt-4o".into();
        assert!(run(&archive, &id, &recipe(1), &changed, Some(&grant), &model).is_err());
        // Same recipe, another server.
        let mut moved = work();
        moved.set_base_url("https://llm.other.example/v1");
        assert!(run(&archive, &id, &recipe(1), &moved, Some(&grant), &model).is_err());
        assert!(model.calls.borrow().is_empty());
        assert!(crate::archive::external::read_log(&archive, &id).unwrap().is_empty());
    }

    /// With a matching grant the run goes out, is logged first (metadata
    /// only) and the document records `external: true` and the host.
    #[test]
    fn a_confirmed_external_run_is_logged_and_marked() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let store = crate::llm::consent::ConsentStore::default();
        let target = RunTarget::new(&id, &recipe(1), &work());
        let grant = store.consume(&store.issue(target.clone()), &target).unwrap();
        let model = FakeModel::new("- Anna manda il file.");
        let out = run(&archive, &id, &recipe(1), &work(), Some(&grant), &model).unwrap();
        assert_eq!(out.file.as_deref(), Some("summary.md"));
        assert_eq!(model.calls.borrow().len(), 1);

        let raw = std::fs::read_to_string(archive.join(&id).join("summary.md")).unwrap();
        assert!(raw.contains("external: true\n") && raw.contains("host: api.example.com\n"), "{raw}");
        let log = crate::archive::external::read_log(&archive, &id).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log[0],
            ExternalSend {
                date: NOW.into(),
                host: "api.example.com".into(),
                profile: "Work".into(),
                model: "gpt".into(),
                kind: SendKind::Recipe,
                recipe: "summary".into(),
            }
        );
        let log_raw = std::fs::read_to_string(archive.join(&id).join(".sussurro/external-log.json")).unwrap();
        assert!(!log_raw.contains("Mando il file"), "the log never holds content");
        assert_eq!(crate::archive::read_item(&archive, &id).unwrap().external_hosts, ["api.example.com"]);

        // A free question is logged as a question, without its text.
        let q = super::super::answer::question_recipe("Chi manda il file?").unwrap();
        let target = RunTarget::new(&id, &q, &work());
        let grant = store.consume(&store.issue(target.clone()), &target).unwrap();
        run(&archive, &id, &q, &work(), Some(&grant), &FakeModel::new("Anna.")).unwrap();
        let log = crate::archive::external::read_log(&archive, &id).unwrap();
        assert_eq!((log[1].kind, log[1].recipe.as_str()), (SendKind::Question, ""));
        let log_raw = std::fs::read_to_string(archive.join(&id).join(".sussurro/external-log.json")).unwrap();
        assert!(!log_raw.contains("Chi manda"), "the log never holds the question");
    }

    /// A local run leaves no trace of an external send.
    #[test]
    fn local_runs_are_not_logged_or_marked() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        run(&archive, &id, &recipe(1), &local(), None, &FakeModel::new("ok")).unwrap();
        let raw = std::fs::read_to_string(archive.join(&id).join("summary.md")).unwrap();
        assert!(raw.contains("external: false\n") && !raw.contains("host:"), "{raw}");
        assert!(crate::archive::external::read_log(&archive, &id).unwrap().is_empty());
        assert!(crate::archive::read_item(&archive, &id).unwrap().external_hosts.is_empty());
    }

    #[test]
    fn preview_says_what_goes_where() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let id = meeting(&archive);
        let p = preview(&archive, &id, &recipe(1), None, &work()).unwrap();
        assert_eq!(p.item_title, "Weekly sync");
        assert_eq!((p.recipe_id.as_str(), p.recipe_name.as_str()), ("summary", "Summary"));
        assert_eq!((p.host.as_str(), p.model.as_str(), p.profile_name.as_str()), ("api.example.com", "gpt", "Work"));
        assert!(p.external);
        let lines = "[00:01:01] Anna: Mando il file venerdì.\n[00:01:10] Ok.\n".chars().count();
        assert_eq!(p.chars, lines + recipe(1).prompt.chars().count());
        assert_eq!(p.approx_tokens, p.chars.div_ceil(4));
        assert!(preview(&archive, "2026/09/nope", &recipe(1), None, &work()).is_err());
        assert_eq!(approx_tokens(0), 0);
        assert_eq!(approx_tokens(9), 3);
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
        run_on_item(&archive, &id, &recipe(1), &local(), None, &model, NOW, &AtomicBool::new(false), &mut |_| {}).unwrap();
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
        assert!(run_on_item(&archive, &id, &recipe(1), &local(), None, &model, NOW, &AtomicBool::new(false), &mut |_| {})
            .is_err());
        assert!(model.calls.borrow().is_empty());
    }

    #[test]
    fn runs_registry_one_per_item_with_cancel() {
        let runs = Runs::default();
        let flag = runs.begin("a", &recipe(0)).unwrap();
        assert!(runs.begin("a", &recipe(1)).is_err());
        assert!(runs.begin_with("b", &recipe(1), Some("Why?")).is_ok());
        runs.set_progress("a", Progress { phase: engine::Phase::Map, done: 1, total: 3 });
        let list = runs.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].progress.unwrap().done, 1);
        assert_eq!((list[0].question.as_deref(), list[1].question.as_deref()), (None, Some("Why?")));
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
            let out = run_on_item(&archive, &id, &r, &profile, None, &model, NOW, &AtomicBool::new(false), &mut |p| {
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
