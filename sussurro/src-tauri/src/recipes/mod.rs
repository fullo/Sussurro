//! Recipes (0.8, #120; plan §4.5): a named prompt with a target — a
//! *companion document* written next to the transcript, or an *answer*
//! shown in the Ask panel (#121). Four are built in; users add their own,
//! stored in `settings.json` ([`crate::settings::Settings::recipes`]).
//!
//! - [`chunk`]: the transcript as prompt input, and chunking by size
//! - [`prompt`]: single, map and reduce prompt assembly
//! - [`engine`]: map-reduce orchestration over a [`engine::ChatModel`]
//! - [`run`]: one run on an archive item — privacy refusal, input,
//!   engine, companion document with provenance

pub mod chunk;
pub mod engine;
pub mod prompt;
pub mod run;

use serde::{Deserialize, Serialize};

/// Where a recipe's output goes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecipeTarget {
    /// A markdown file next to `transcript.md`.
    #[default]
    CompanionDocument,
    /// Text shown in the Ask panel (#121), savable as a document there.
    Answer,
}

/// A named prompt with a target (plan §5).
///
/// Every field has a default so a hand-edited recipe never makes the whole
/// settings file unreadable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    /// The task, in plain words ("List the action items…"). The transcript
    /// and the output rules are added by [`prompt`].
    pub prompt: String,
    pub target: RecipeTarget,
    /// Shipped with the app: read-only, never stored in the settings.
    pub builtin: bool,
}

/// Id of the *Formatted document* recipe, which writes `document.md`.
pub const FORMATTED_DOCUMENT_ID: &str = "formatted-document";

const FORMATTED_DOCUMENT_PROMPT: &str = "\
Turn the transcript into a well-structured written document in markdown.
- Start with one line that begins with `**tl;dr:**` followed by one or two sentences with the gist.
- Then a `#` title, and organise the content under `##` to `######` headings that follow its topics, in order.
- Rewrite speech into clear written prose: drop repetitions and false starts, keep every fact, name, number and decision.
- Where the content is tabular (options compared, items with owners or dates, figures), use a markdown table.
- Use bullet lists for enumerations.";

const SUMMARY_PROMPT: &str = "\
Summarise the transcript in markdown: a short paragraph with the gist, then the key points as a bullet list. \
Keep names, numbers and decisions exact.";

const ACTION_ITEMS_PROMPT: &str = "\
List every action item in the transcript as a markdown task list (`- [ ] …`). For each one say what has to be done, \
and who owns it and by when if that is said (for example `— Anna, by Friday`). \
If there are none, write one line saying there are no action items.";

const DECISIONS_PROMPT: &str = "\
List the decisions taken in the transcript as a markdown bullet list, each with its reason and who took it when that \
is said. Leave out proposals that were not agreed. If there are none, write one line saying no decisions were taken.";

/// The recipes shipped with the app, in display order.
pub fn builtin_recipes() -> Vec<Recipe> {
    let r = |id: &str, name: &str, prompt: &str| Recipe {
        id: id.into(),
        name: name.into(),
        prompt: prompt.into(),
        target: RecipeTarget::CompanionDocument,
        builtin: true,
    };
    vec![
        r(FORMATTED_DOCUMENT_ID, "Formatted document", FORMATTED_DOCUMENT_PROMPT),
        r("summary", "Summary", SUMMARY_PROMPT),
        r("action-items", "Action items", ACTION_ITEMS_PROMPT),
        r("decisions", "Decisions", DECISIONS_PROMPT),
    ]
}

/// Built-ins first, then the user's recipes.
pub fn all_recipes(user: &[Recipe]) -> Vec<Recipe> {
    let mut all = builtin_recipes();
    all.extend(user.iter().cloned());
    all
}

/// A recipe by id, built-in or the user's.
pub fn find_recipe(user: &[Recipe], id: &str) -> Option<Recipe> {
    all_recipes(user).into_iter().find(|r| r.id == id)
}

/// Bring the user's recipes into a usable shape: never marked built-in,
/// names trimmed (empty → "Untitled recipe"), ids unique and distinct from
/// the built-ins (empty or clashing → `recipe-N`). Nothing is dropped: a
/// recipe with an empty prompt is kept for the user to finish. Idempotent.
pub fn normalize_user_recipes(recipes: &mut [Recipe]) {
    use std::collections::HashSet;
    let builtin: HashSet<String> = builtin_recipes().into_iter().map(|r| r.id).collect();
    let all: HashSet<String> = recipes.iter().map(|r| r.id.trim().to_string()).collect();
    let mut seen = HashSet::new();
    let mut next = 1;
    for r in recipes.iter_mut() {
        r.builtin = false;
        r.id = r.id.trim().to_string();
        r.name = r.name.trim().to_string();
        if r.name.is_empty() {
            r.name = "Untitled recipe".into();
        }
        if r.id.is_empty() || builtin.contains(&r.id) || !seen.insert(r.id.clone()) {
            loop {
                let candidate = format!("recipe-{next}");
                next += 1;
                if !all.contains(&candidate) && !seen.contains(&candidate) {
                    r.id = candidate;
                    break;
                }
            }
            seen.insert(r.id.clone());
        }
    }
}

/// File a companion-document recipe writes: `document.md` for *Formatted
/// document*, `<slug of the name>.md` for the others (`action-items.md`).
/// A slug that would name the transcript or the formatted document gets a
/// `recipe-` prefix, so a user recipe can never take their place.
pub fn companion_file_name(recipe: &Recipe) -> String {
    if recipe.id == FORMATTED_DOCUMENT_ID {
        return crate::archive::companion::DOCUMENT_FILE.to_string();
    }
    let slug = crate::archive::paths::slugify(&recipe.name);
    match slug.as_str() {
        "transcript" | "document" => format!("recipe-{slug}.md"),
        _ => format!("{slug}.md"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_builtins_all_writing_documents() {
        let b = builtin_recipes();
        let names: Vec<_> = b.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Formatted document", "Summary", "Action items", "Decisions"]);
        assert!(b.iter().all(|r| r.builtin && r.target == RecipeTarget::CompanionDocument));
        assert!(b.iter().all(|r| !r.prompt.trim().is_empty()));
        let fd = &b[0].prompt;
        assert!(fd.contains("tl;dr") && fd.contains("######") && fd.contains("table"));
    }

    #[test]
    fn companion_file_names() {
        let b = builtin_recipes();
        let files: Vec<_> = b.iter().map(companion_file_name).collect();
        assert_eq!(files, ["document.md", "summary.md", "action-items.md", "decisions.md"]);
        let user = |name: &str| Recipe { id: "u".into(), name: name.into(), ..Default::default() };
        assert_eq!(companion_file_name(&user("Domande aperte")), "domande-aperte.md");
        assert_eq!(companion_file_name(&user("Transcript")), "recipe-transcript.md");
        assert_eq!(companion_file_name(&user("Document")), "recipe-document.md");
        assert_eq!(companion_file_name(&user("??")), "untitled.md");
    }

    #[test]
    fn normalize_user_recipes_repairs_ids_names_and_flags() {
        let mut r = vec![
            Recipe { id: "summary".into(), name: "Mine".into(), builtin: true, ..Default::default() },
            Recipe { id: "q".into(), name: " Q ".into(), ..Default::default() },
            Recipe { id: "q".into(), name: "".into(), ..Default::default() },
            Recipe { id: "recipe-1".into(), name: "R".into(), ..Default::default() },
        ];
        normalize_user_recipes(&mut r);
        let ids: Vec<_> = r.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["recipe-2", "q", "recipe-3", "recipe-1"]);
        assert!(r.iter().all(|x| !x.builtin));
        assert_eq!(r[1].name, "Q");
        assert_eq!(r[2].name, "Untitled recipe");
        let before = r.clone();
        normalize_user_recipes(&mut r);
        assert_eq!(r, before, "idempotent");
    }

    #[test]
    fn find_covers_builtins_and_user_recipes() {
        let user = vec![Recipe { id: "mine".into(), name: "Mine".into(), ..Default::default() }];
        assert_eq!(find_recipe(&user, "summary").unwrap().name, "Summary");
        assert_eq!(find_recipe(&user, "mine").unwrap().name, "Mine");
        assert!(find_recipe(&user, "nope").is_none());
        assert_eq!(all_recipes(&user).len(), 5);
    }

    #[test]
    fn recipe_serde_is_lenient_and_snake_case() {
        let r: Recipe = serde_json::from_str(r#"{"name":"X","target":"answer"}"#).unwrap();
        assert_eq!(r.target, RecipeTarget::Answer);
        assert!(r.id.is_empty() && !r.builtin);
        let json = serde_json::to_value(Recipe::default()).unwrap();
        assert_eq!(json["target"], "companion_document");
    }
}
