//! Per-run consent for external LLM profiles (0.8, #122; plan §4.5 "Privacy
//! gate").
//!
//! A recipe or a free question on an external profile sends the transcript
//! off this machine, so it needs the user's explicit OK **every time**. The
//! backend enforces it, not the UI: after the user confirms in the dialog,
//! the UI asks for a one-time token (`prepare_external_run`), and the run
//! command only sends when it gets a token that
//!
//! - was issued by this process and not used before (single use: consuming
//!   it removes it, whether the run then starts or not),
//! - is younger than [`CONSENT_TTL`], and
//! - was issued for exactly this run ([`RunTarget`]): the same item, the
//!   same recipe (or question), the same profile, host and model. Editing
//!   the profile's server or model after confirming voids it.
//!
//! A run on an external profile without such a token is refused before
//! anything is sent. Local profiles need no token.
//!
//! Cleanup (dictation, long-form runs, `/clean`) has no dialog moment: it
//! uses a persistent per-profile opt-in instead
//! ([`crate::llm::LlmProfile::cleanup_allowed`]).

use crate::llm::LlmProfile;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a confirmation stays valid: the run starts right after the
/// click, so a short window is plenty.
pub const CONSENT_TTL: Duration = Duration::from_secs(120);
/// Tokens kept at most (oldest dropped first): a confirmation is used at
/// once, so only a runaway caller would pile them up.
const MAX_PENDING: usize = 16;

/// Exactly what a confirmation allows: this task on this item, sent to this
/// host with this model through this profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTarget {
    pub item_id: String,
    /// The recipe id plus a digest of its prompt: a user recipe edited (or
    /// another question asked) after confirming is another task.
    pub task: String,
    pub profile_id: String,
    pub host: String,
    pub model: String,
}

impl RunTarget {
    /// The target of running `recipe` on item `item_id` with `profile`.
    pub fn new(item_id: &str, recipe: &crate::recipes::Recipe, profile: &LlmProfile) -> Self {
        let digest = crate::archive::store::sha256_hex(recipe.prompt.as_bytes());
        Self {
            item_id: item_id.to_string(),
            task: format!("{}#{}", recipe.id, &digest[..16]),
            profile_id: profile.id.clone(),
            host: profile.host(),
            model: profile.model.trim().to_string(),
        }
    }
}

/// Proof that the user confirmed a run: obtainable only by consuming a
/// valid token ([`ConsentStore::consume`]), so holding one means the check
/// passed. Carries the target it was issued for.
#[derive(Debug)]
pub struct ConsentGrant {
    target: RunTarget,
}

impl ConsentGrant {
    pub fn target(&self) -> &RunTarget {
        &self.target
    }
}

/// Pending tokens: what each was issued for, and when.
type Pending = HashMap<String, (RunTarget, Instant)>;

/// Confirmation tokens not used yet, in memory only (a restart forgets them).
#[derive(Default)]
pub struct ConsentStore {
    inner: Mutex<(u64, Pending)>,
}

impl ConsentStore {
    /// Issue a one-time token for `target` (the user just confirmed).
    pub fn issue(&self, target: RunTarget) -> String {
        self.issue_at(target, Instant::now())
    }

    pub(crate) fn issue_at(&self, target: RunTarget, now: Instant) -> String {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.0 += 1;
        let token = new_token(g.0);
        g.1.retain(|_, (_, at)| now.saturating_duration_since(*at) < CONSENT_TTL);
        while g.1.len() >= MAX_PENDING {
            let oldest = g.1.iter().min_by_key(|(_, (_, at))| *at).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => g.1.remove(&k),
                None => break,
            };
        }
        g.1.insert(token.clone(), (target, now));
        token
    }

    /// Use `token` for a run on `target`. The token is gone afterwards
    /// whatever the outcome (single use; a mismatch burns it too).
    pub fn consume(&self, token: &str, target: &RunTarget) -> Result<ConsentGrant> {
        self.consume_at(token, target, Instant::now())
    }

    pub(crate) fn consume_at(&self, token: &str, target: &RunTarget, now: Instant) -> Result<ConsentGrant> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some((issued_for, at)) = g.1.remove(token) else {
            bail!("this confirmation is not valid (already used or never given) — confirm the run again");
        };
        if now.saturating_duration_since(at) >= CONSENT_TTL {
            bail!("this confirmation expired — confirm the run again");
        }
        if issued_for != *target {
            bail!(
                "this confirmation was given for another run (item, recipe, profile, server or model changed) — \
                 confirm the run again"
            );
        }
        Ok(ConsentGrant { target: issued_for })
    }

    #[cfg(test)]
    fn pending(&self) -> usize {
        self.inner.lock().unwrap().1.len()
    }
}

/// An unguessable token: SHA-256 over two SipHash outputs keyed from the
/// OS's randomness (std's `RandomState`), a counter, the clock and the pid.
/// No extra dependency; the token never leaves the app (IPC only).
fn new_token(counter: u64) -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut seed = Vec::with_capacity(48);
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(counter ^ (i << 63));
        seed.extend_from_slice(&h.finish().to_le_bytes());
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    seed.extend_from_slice(&nanos.to_le_bytes());
    seed.extend_from_slice(&counter.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    crate::archive::store::sha256_hex(&seed)
}

/// The consent a run needs: none for a local profile; for an external one a
/// grant issued for exactly this run.
pub fn authorize(profile: &LlmProfile, target: &RunTarget, grant: Option<&ConsentGrant>) -> Result<()> {
    if !profile.external {
        return Ok(());
    }
    match grant {
        None => bail!(
            "“{}” is an external profile: the transcript would go to {}. Confirm the run first — nothing was sent.",
            profile.name,
            profile.host_label()
        ),
        Some(g) if g.target() != target => bail!(
            "the confirmation does not match this run (item, recipe, profile, server or model changed) — \
             nothing was sent; confirm the run again"
        ),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipes::builtin_recipes;
    use crate::settings::CleanupApi;

    fn work() -> LlmProfile {
        LlmProfile::new("work", "Work", CleanupApi::Openai, "https://api.example.com/v1", "k", "gpt-4o-mini")
    }

    fn target(item: &str) -> RunTarget {
        RunTarget::new(item, &builtin_recipes()[1], &work())
    }

    #[test]
    fn a_token_is_single_use() {
        let store = ConsentStore::default();
        let t = store.issue(target("a"));
        assert_eq!(t.len(), 64);
        let grant = store.consume(&t, &target("a")).unwrap();
        assert_eq!(grant.target().host, "api.example.com");
        assert_eq!(grant.target().model, "gpt-4o-mini");
        let err = store.consume(&t, &target("a")).unwrap_err();
        assert!(format!("{err}").contains("already used"), "{err}");
        assert!(store.consume("made-up", &target("a")).is_err());
    }

    #[test]
    fn tokens_are_distinct() {
        let store = ConsentStore::default();
        let a = store.issue(target("a"));
        let b = store.issue(target("a"));
        assert_ne!(a, b);
        assert_eq!(store.pending(), 2);
    }

    #[test]
    fn a_token_expires() {
        let store = ConsentStore::default();
        let t0 = Instant::now();
        let t = store.issue_at(target("a"), t0);
        let err = store.consume_at(&t, &target("a"), t0 + CONSENT_TTL).unwrap_err();
        assert!(format!("{err}").contains("expired"), "{err}");
        // Just inside the window it works.
        let t = store.issue_at(target("a"), t0);
        assert!(store.consume_at(&t, &target("a"), t0 + CONSENT_TTL - Duration::from_secs(1)).is_ok());
        // Expired tokens are dropped when new ones are issued.
        store.issue_at(target("b"), t0);
        store.issue_at(target("c"), t0 + CONSENT_TTL * 2);
        assert_eq!(store.pending(), 1);
    }

    #[test]
    fn a_token_is_bound_to_item_task_profile_host_and_model() {
        let store = ConsentStore::default();
        let recipes = builtin_recipes();
        let base = target("a");
        let mut other_profile = work();
        other_profile.id = "work-2".into();
        let mut other_host = work();
        other_host.base_url = "https://llm.other.example/v1".into();
        let mut other_model = work();
        other_model.model = "gpt-4o".into();
        let mut edited = recipes[1].clone();
        edited.prompt.push_str(" In English.");
        for wrong in [
            target("b"),
            RunTarget::new("a", &recipes[2], &work()),
            RunTarget::new("a", &edited, &work()),
            RunTarget::new("a", &recipes[1], &other_profile),
            RunTarget::new("a", &recipes[1], &other_host),
            RunTarget::new("a", &recipes[1], &other_model),
        ] {
            let t = store.issue(base.clone());
            let err = store.consume(&t, &wrong).unwrap_err();
            assert!(format!("{err}").contains("another run"), "{err}");
            // A mismatch burns the token: it can't be retried on the right run.
            assert!(store.consume(&t, &base).is_err());
        }
    }

    #[test]
    fn questions_are_bound_to_their_text() {
        let q1 = crate::recipes::answer::question_recipe("Who sends the file?").unwrap();
        let q2 = crate::recipes::answer::question_recipe("When is the deadline?").unwrap();
        assert_ne!(RunTarget::new("a", &q1, &work()), RunTarget::new("a", &q2, &work()));
        assert_eq!(RunTarget::new("a", &q1, &work()), RunTarget::new("a", &q1.clone(), &work()));
    }

    #[test]
    fn pending_tokens_are_bounded() {
        let store = ConsentStore::default();
        let first = store.issue(target("a"));
        for _ in 0..MAX_PENDING {
            store.issue(target("a"));
        }
        assert_eq!(store.pending(), MAX_PENDING);
        assert!(store.consume(&first, &target("a")).is_err(), "the oldest was dropped");
    }

    #[test]
    fn authorize_needs_a_matching_grant_only_for_external_profiles() {
        let local = LlmProfile::default();
        let recipe = &builtin_recipes()[0];
        assert!(authorize(&local, &RunTarget::new("a", recipe, &local), None).is_ok());

        let t = RunTarget::new("a", recipe, &work());
        let err = authorize(&work(), &t, None).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("api.example.com") && msg.contains("nothing was sent"), "{msg}");

        let store = ConsentStore::default();
        let grant = store.consume(&store.issue(t.clone()), &t).unwrap();
        assert!(authorize(&work(), &t, Some(&grant)).is_ok());
        let elsewhere = RunTarget::new("b", recipe, &work());
        assert!(authorize(&work(), &elsewhere, Some(&grant)).is_err());
    }
}
