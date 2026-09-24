//! LLM profile API keys in the OS credential store (#159): the macOS
//! Keychain, the Windows Credential Manager, or the Secret Service on Linux
//! (GNOME Keyring, KWallet, KeePassXC…), through the `keyring-core` crate
//! and one native store crate per OS.
//!
//! `settings.json` keeps only a reference ([`KeyStorage::Keychain`] on the
//! profile); the entry is `service` [`SERVICE`], `user` `llm-profile:<id>`.
//! Where no store works (a headless Linux box without a Secret Service, a
//! keychain that refuses the write) the key falls back to clear text in
//! `settings.json` ([`KeyStorage::File`]) and the profile editor says so.
//!
//! The logic ([`load_keys`] at start, [`sync_keys`] on every save) takes the
//! store as a [`SecretStore`], so tests run it on an in-memory fake and never
//! touch the real keychain. Error messages never carry a key.

use crate::llm::{KeyStorage, LlmProfile};
use crate::settings::Settings;
use std::sync::{Arc, Mutex};

/// Service name of every entry — the app's bundle identifier, so the
/// entries are recognizable in Keychain Access / Credential Manager /
/// Seahorse.
pub const SERVICE: &str = "com.sussurro.app";

/// Credential-store account holding a profile's key.
pub fn account(profile_id: &str) -> String {
    format!("llm-profile:{profile_id}")
}

/// A failed credential-store operation. The message names the operation and
/// the store's reason, never the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The three operations the profile keys need.
pub trait SecretStore {
    /// The secret of `account`, `None` when there is no entry.
    fn get(&self, account: &str) -> Result<Option<String>, StoreError>;
    /// Create or replace the entry of `account`.
    fn set(&self, account: &str, secret: &str) -> Result<(), StoreError>;
    /// Remove the entry of `account`; a missing entry is not an error.
    fn delete(&self, account: &str) -> Result<(), StoreError>;
}

/// The OS credential store. The native store is built on first use and
/// kept; while building fails (no Secret Service yet), every call tries
/// again, so a store that appears later is picked up.
pub struct OsStore;

type NativeStore = Arc<keyring_core::CredentialStore>;

fn native_store() -> Result<NativeStore, StoreError> {
    static STORE: Mutex<Option<NativeStore>> = Mutex::new(None);
    let mut slot = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(store) = slot.as_ref() {
        return Ok(store.clone());
    }
    let store = build_native_store().map_err(|e| StoreError(format!("no credential store: {}", describe(&e))))?;
    *slot = Some(store.clone());
    Ok(store)
}

#[cfg(target_os = "macos")]
fn build_native_store() -> keyring_core::Result<NativeStore> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "windows")]
fn build_native_store() -> keyring_core::Result<NativeStore> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(target_os = "linux")]
fn build_native_store() -> keyring_core::Result<NativeStore> {
    Ok(dbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn build_native_store() -> keyring_core::Result<NativeStore> {
    Err(keyring_core::Error::NotSupportedByStore(
        "no credential store on this platform".into(),
    ))
}

/// A store error in words, without any secret material (the variants that
/// carry raw secret bytes are described, not printed).
fn describe(e: &keyring_core::Error) -> String {
    use keyring_core::Error as E;
    match e {
        E::PlatformFailure(err) => format!("platform failure: {err}"),
        E::NoStorageAccess(err) => format!("store not accessible (locked?): {err}"),
        E::NoEntry => "no entry".into(),
        E::BadEncoding(_) => "stored key is not UTF-8".into(),
        E::BadDataFormat(..) => "stored key is malformed".into(),
        E::Ambiguous(items) => format!("{} matching entries", items.len()),
        other => {
            // Remaining variants carry attribute names and reasons only.
            other.to_string()
        }
    }
}

impl OsStore {
    fn entry(account: &str) -> Result<keyring_core::Entry, StoreError> {
        native_store()?
            .build(SERVICE, account, None)
            .map_err(|e| StoreError(describe(&e)))
    }
}

impl SecretStore for OsStore {
    fn get(&self, account: &str) -> Result<Option<String>, StoreError> {
        match Self::entry(account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(StoreError(describe(&e))),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), StoreError> {
        Self::entry(account)?
            .set_password(secret)
            .map_err(|e| StoreError(describe(&e)))
    }

    fn delete(&self, account: &str) -> Result<(), StoreError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(StoreError(describe(&e))),
        }
    }
}

/// Whether the credential store answers at all: a read of an entry that
/// does not exist (no prompt on any OS). `Err` carries the reason.
pub fn probe(store: &dyn SecretStore) -> Result<(), StoreError> {
    store.get(&account("__probe__")).map(|_| ())
}

/// What the profile editor shows about the credential store.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct StoreStatus {
    /// A key saved now goes to the credential store.
    pub available: bool,
    /// The store's name on this OS, for messages.
    pub name: &'static str,
    /// Why it is unavailable (empty when available).
    pub error: String,
}

/// This OS's credential store, in words.
pub fn store_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "the macOS Keychain"
    } else if cfg!(target_os = "windows") {
        "Windows Credential Manager"
    } else {
        "the Secret Service keyring"
    }
}

pub fn status(store: &dyn SecretStore) -> StoreStatus {
    let result = probe(store);
    StoreStatus {
        available: result.is_ok(),
        name: store_name(),
        error: result.err().map(|e| e.0).unwrap_or_default(),
    }
}

/// Put `p`'s key in the store and read it back: only a verified write
/// counts (the clear-text copy is dropped from disk after this succeeds).
fn store_key(store: &dyn SecretStore, p: &LlmProfile) -> Result<(), StoreError> {
    let acct = account(&p.id);
    store.set(&acct, &p.api_key)?;
    match store.get(&acct)? {
        Some(read) if read == p.api_key => Ok(()),
        _ => Err(StoreError("the key read back from the store does not match".into())),
    }
}

/// At start, after [`Settings::load_migrating`]: fill in the keys kept in
/// the credential store and move clear-text keys into it. Returns true when
/// `settings.json` must be rewritten (a key left the file, or a reference
/// changed).
///
/// - `keychain`: read the key. Unreadable (store locked, prompt denied) →
///   [`KeyStorage::Unreadable`] for this session; the reference stays on
///   disk. Entry gone (removed by hand) → no key.
/// - A clear-text key (a file from before #159, or a `file` fallback from an
///   earlier run): written to the store and verified, then dropped from the
///   file. If the store fails it stays in the file as [`KeyStorage::File`].
pub fn load_keys(settings: &mut Settings, store: &dyn SecretStore) -> bool {
    let mut dirty = false;
    for p in &mut settings.llm_profiles {
        if p.api_key_storage.in_store() {
            match store.get(&account(&p.id)) {
                Ok(Some(key)) => {
                    // A stray clear-text copy (hand-edited file) goes.
                    dirty |= !p.api_key.is_empty();
                    p.api_key = key;
                    p.api_key_storage = KeyStorage::Keychain;
                    continue;
                }
                Ok(None) if p.api_key.is_empty() => {
                    eprintln!("LLM profile “{}”: its API key is no longer in the credential store", p.name);
                    p.api_key_storage = KeyStorage::None;
                    dirty = true;
                    continue;
                }
                // Entry gone but a clear-text copy is in the file: migrate it below.
                Ok(None) => p.api_key_storage = KeyStorage::None,
                Err(e) => {
                    eprintln!("LLM profile “{}”: could not read its API key from the credential store: {e}", p.name);
                    // A stray clear-text copy is still usable (and already on disk).
                    p.api_key_storage = if p.api_key.is_empty() {
                        KeyStorage::Unreadable
                    } else {
                        KeyStorage::File
                    };
                    continue;
                }
            }
        }
        if p.api_key.is_empty() {
            if p.api_key_storage != KeyStorage::None {
                p.api_key_storage = KeyStorage::None;
                dirty = true;
            }
            continue;
        }
        match store_key(store, p) {
            Ok(()) => {
                p.api_key_storage = KeyStorage::Keychain;
                dirty = true;
            }
            Err(e) => {
                if p.api_key_storage != KeyStorage::File {
                    eprintln!(
                        "LLM profile “{}”: no credential store ({e}); its API key stays in settings.json",
                        p.name
                    );
                    p.api_key_storage = KeyStorage::File;
                    dirty = true;
                }
            }
        }
    }
    dirty
}

/// On save from the UI (`set_settings`): bring the credential store in line
/// with `next`, given the settings in use until now (`prev`). The UI's
/// `api_key_storage` is ignored — it is recomputed here.
///
/// - Key unchanged: nothing touches the store; the previous storage holds
///   (so a key that could not be read this session is not deleted).
/// - Key changed or new profile: written to the store ([`KeyStorage::Keychain`]),
///   or kept in clear text when that fails ([`KeyStorage::File`]).
/// - Key cleared: its entry is deleted.
/// - Profile deleted: its entry is deleted.
pub fn sync_keys(next: &mut Settings, prev: &Settings, store: &dyn SecretStore) {
    for p in &mut next.llm_profiles {
        let old = prev.llm_profiles.iter().find(|o| o.id == p.id);
        if let Some(o) = old {
            if o.api_key == p.api_key {
                p.api_key_storage = o.api_key_storage;
                continue;
            }
        }
        let old_in_store = old.is_some_and(|o| o.api_key_storage.in_store());
        if p.api_key.is_empty() {
            if old_in_store {
                if let Err(e) = store.delete(&account(&p.id)) {
                    eprintln!("LLM profile “{}”: could not delete its API key from the credential store: {e}", p.name);
                }
            }
            p.api_key_storage = KeyStorage::None;
            continue;
        }
        p.api_key_storage = match store.set(&account(&p.id), &p.api_key) {
            Ok(()) => KeyStorage::Keychain,
            Err(e) => {
                eprintln!(
                    "LLM profile “{}”: no credential store ({e}); its API key is saved in settings.json",
                    p.name
                );
                // The store's entry (if any) holds the old key: remove it
                // so a later start doesn't bring it back.
                if old_in_store {
                    let _ = store.delete(&account(&p.id));
                }
                KeyStorage::File
            }
        };
    }
    for o in &prev.llm_profiles {
        if o.api_key_storage.in_store() && !next.llm_profiles.iter().any(|p| p.id == o.id) {
            if let Err(e) = store.delete(&account(&o.id)) {
                eprintln!("deleted LLM profile “{}”: could not delete its API key from the credential store: {e}", o.name);
            }
        }
    }
}

/// Where a profile's key is, for diagnostics — never the key.
pub fn key_summary(p: &LlmProfile) -> &'static str {
    match p.api_key_storage {
        KeyStorage::Keychain => "in the OS credential store",
        KeyStorage::Unreadable => "in the OS credential store (unreadable this session)",
        KeyStorage::File => "in settings.json (no credential store)",
        KeyStorage::None if p.api_key.is_empty() => "none",
        KeyStorage::None => "not stored yet",
    }
}

/// A server address for diagnostics: credentials in the URL (`user:pass@`)
/// and the query string (`?key=…`, as some hosted APIs take) are masked.
pub fn redact_url(url: &str) -> String {
    let s = url.trim();
    let (scheme, rest) = match s.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), s),
    };
    let (authority, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let authority = match authority.rfind('@') {
        Some(at) => format!("…@{}", &authority[at + 1..]),
        None => authority.to_string(),
    };
    let tail = match tail.find(['?', '#']) {
        Some(i) => format!("{}?…", &tail[..i]),
        None => tail.to_string(),
    };
    format!("{scheme}{authority}{tail}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::settings::CleanupApi;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    /// In-memory credential store; `broken` makes every call fail like a
    /// headless Linux box without a Secret Service, `locked` fails reads
    /// only (a locked keychain / denied prompt).
    #[derive(Default)]
    pub(crate) struct FakeStore {
        pub entries: RefCell<HashMap<String, String>>,
        pub broken: Cell<bool>,
        pub locked: Cell<bool>,
        pub writes: Cell<usize>,
    }

    impl SecretStore for FakeStore {
        fn get(&self, account: &str) -> Result<Option<String>, StoreError> {
            if self.broken.get() || self.locked.get() {
                return Err(StoreError("unavailable".into()));
            }
            Ok(self.entries.borrow().get(account).cloned())
        }
        fn set(&self, account: &str, secret: &str) -> Result<(), StoreError> {
            if self.broken.get() {
                return Err(StoreError("unavailable".into()));
            }
            self.writes.set(self.writes.get() + 1);
            self.entries.borrow_mut().insert(account.into(), secret.into());
            Ok(())
        }
        fn delete(&self, account: &str) -> Result<(), StoreError> {
            if self.broken.get() {
                return Err(StoreError("unavailable".into()));
            }
            self.entries.borrow_mut().remove(account);
            Ok(())
        }
    }

    fn work(key: &str) -> LlmProfile {
        LlmProfile::new("work", "Work", CleanupApi::Openai, "https://api.example.com/v1", key, "gpt")
    }

    fn with_profiles(profiles: Vec<LlmProfile>) -> Settings {
        Settings { llm_profiles: profiles, ..Default::default() }
    }

    /// The settings.json a pre-#159 build wrote, with a clear-text key.
    const CLEAR_TEXT: &str = r#"{
  "llm_profiles": [
    {"id": "local", "name": "Local", "api": "ollama", "base_url": "http://localhost:11434", "api_key": "", "model": "llama3.2:3b", "external": false},
    {"id": "work", "name": "Work", "api": "openai", "base_url": "https://api.example.com/v1", "api_key": "sk-secret-123", "model": "gpt", "external": true}
  ],
  "cleanup_profile": "work"
}"#;

    fn saved(path: &std::path::Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    /// Acceptance criterion: a clear-text key moves to the store on the
    /// first start, and settings.json keeps only the reference.
    #[test]
    fn clear_text_key_migrates_to_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, CLEAR_TEXT).unwrap();
        let store = FakeStore::default();

        let (mut s, _) = Settings::load_migrating(&path);
        assert!(load_keys(&mut s, &store), "the file must be rewritten");
        assert_eq!(store.entries.borrow().get("llm-profile:work").unwrap(), "sk-secret-123");
        // In memory the key is still there: cleanup keeps working.
        assert_eq!(s.cleanup_llm().api_key, "sk-secret-123");
        assert_eq!(s.cleanup_llm().api_key_storage, KeyStorage::Keychain);
        // The local profile has no key and is left alone.
        assert_eq!(s.llm_profiles[0].api_key_storage, KeyStorage::None);

        s.save(&path).unwrap();
        let text = saved(&path);
        assert!(!text.contains("sk-secret-123"), "{text}");
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["llm_profiles"][1]["api_key"], "");
        assert_eq!(json["llm_profiles"][1]["api_key_storage"], "keychain");
        assert!(json["llm_profiles"][0].get("api_key_storage").is_none());

        // Next start: the key comes back from the store, nothing to rewrite.
        let (mut again, _) = Settings::load_migrating(&path);
        assert!(!load_keys(&mut again, &store));
        assert_eq!(again, s);
        assert_eq!(store.writes.get(), 1);
    }

    /// A pre-0.8 flat `api_key` goes through the #119 migration into the
    /// Local profile, then into the store.
    #[test]
    fn legacy_flat_key_migrates_through_both_steps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"cleanup_api":"openai","ollama_url":"http://localhost:8080/v1","api_key":"sk-old"}"#).unwrap();
        let store = FakeStore::default();
        let (mut s, migrated) = Settings::load_migrating(&path);
        assert!(migrated);
        assert!(load_keys(&mut s, &store));
        assert_eq!(store.entries.borrow().get("llm-profile:local").unwrap(), "sk-old");
        s.save(&path).unwrap();
        assert!(!saved(&path).contains("sk-old"));
    }

    /// No credential store (headless Linux): the key stays in settings.json,
    /// marked as the fallback so the editor warns; it is never lost.
    #[test]
    fn without_a_store_the_key_falls_back_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, CLEAR_TEXT).unwrap();
        let store = FakeStore::default();
        store.broken.set(true);

        let (mut s, _) = Settings::load_migrating(&path);
        load_keys(&mut s, &store);
        let w = &s.llm_profiles[1];
        assert_eq!(w.api_key, "sk-secret-123");
        assert_eq!(w.api_key_storage, KeyStorage::File);
        s.save(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&saved(&path)).unwrap();
        assert_eq!(json["llm_profiles"][1]["api_key"], "sk-secret-123");
        assert_eq!(json["llm_profiles"][1]["api_key_storage"], "file");

        // Nothing changes while the store is still missing…
        let (mut again, _) = Settings::load_migrating(&path);
        assert!(!load_keys(&mut again, &store));
        // …and the key moves in once a store shows up.
        store.broken.set(false);
        assert!(load_keys(&mut again, &store));
        assert_eq!(again.llm_profiles[1].api_key_storage, KeyStorage::Keychain);
        again.save(&path).unwrap();
        assert!(!saved(&path).contains("sk-secret-123"));
    }

    /// A write that doesn't read back is not trusted: the key stays in the file.
    #[test]
    fn migration_needs_a_verified_write() {
        struct Lossy;
        impl SecretStore for Lossy {
            fn get(&self, _: &str) -> Result<Option<String>, StoreError> {
                Ok(None)
            }
            fn set(&self, _: &str, _: &str) -> Result<(), StoreError> {
                Ok(())
            }
            fn delete(&self, _: &str) -> Result<(), StoreError> {
                Ok(())
            }
        }
        let mut s = with_profiles(vec![work("sk-1")]);
        load_keys(&mut s, &Lossy);
        assert_eq!(s.llm_profiles[0].api_key_storage, KeyStorage::File);
        assert!(serde_json::to_string(&s.for_disk()).unwrap().contains("sk-1"));
    }

    /// A locked keychain at start: the key is unknown this session, but the
    /// reference stays on disk and a save without a new key keeps the entry.
    #[test]
    fn unreadable_key_keeps_its_entry() {
        let store = FakeStore::default();
        store.entries.borrow_mut().insert(account("work"), "sk-1".into());
        store.locked.set(true);
        let mut disk = work("");
        disk.api_key_storage = KeyStorage::Keychain;
        let mut s = with_profiles(vec![LlmProfile::default(), disk]);

        assert!(!load_keys(&mut s, &store));
        assert_eq!(s.llm_profiles[1].api_key, "");
        assert_eq!(s.llm_profiles[1].api_key_storage, KeyStorage::Unreadable);
        let json = serde_json::to_value(s.for_disk()).unwrap();
        assert_eq!(json["llm_profiles"][1]["api_key_storage"], "keychain");

        // The UI saves something else: the entry survives.
        let mut next = s.clone();
        next.hotkey = "Alt+Space".into();
        next.llm_profiles[1].api_key_storage = KeyStorage::None; // UI value is ignored
        sync_keys(&mut next, &s, &store);
        assert_eq!(next.llm_profiles[1].api_key_storage, KeyStorage::Unreadable);
        assert_eq!(store.entries.borrow().get("llm-profile:work").unwrap(), "sk-1");

        // Unlocked at the next start, the key is back.
        store.locked.set(false);
        let mut later = with_profiles(next.for_disk().llm_profiles);
        load_keys(&mut later, &store);
        assert_eq!(later.llm_profiles[1].api_key, "sk-1");
    }

    /// An entry removed by hand in the OS: the profile has no key any more.
    #[test]
    fn a_missing_entry_drops_the_reference() {
        let store = FakeStore::default();
        let mut disk = work("");
        disk.api_key_storage = KeyStorage::Keychain;
        let mut s = with_profiles(vec![disk]);
        assert!(load_keys(&mut s, &store));
        assert_eq!(s.llm_profiles[0].api_key_storage, KeyStorage::None);
    }

    #[test]
    fn sync_writes_new_and_changed_keys_and_deletes_cleared_ones() {
        let store = FakeStore::default();
        let prev = with_profiles(vec![LlmProfile::default()]);

        // New profile with a key.
        let mut next = with_profiles(vec![LlmProfile::default(), work("sk-1")]);
        sync_keys(&mut next, &prev, &store);
        assert_eq!(next.llm_profiles[1].api_key_storage, KeyStorage::Keychain);
        assert_eq!(store.entries.borrow().get("llm-profile:work").unwrap(), "sk-1");
        assert!(!serde_json::to_string(&next.for_disk()).unwrap().contains("sk-1"));

        // Unchanged key: no store write.
        let writes = store.writes.get();
        let mut same = next.clone();
        same.hotkey = "Alt+Space".into();
        sync_keys(&mut same, &next, &store);
        assert_eq!(store.writes.get(), writes);
        assert_eq!(same.llm_profiles[1].api_key_storage, KeyStorage::Keychain);

        // Changed key.
        let mut changed = same.clone();
        changed.llm_profiles[1].api_key = "sk-2".into();
        sync_keys(&mut changed, &same, &store);
        assert_eq!(store.entries.borrow().get("llm-profile:work").unwrap(), "sk-2");

        // Cleared key: entry deleted.
        let mut cleared = changed.clone();
        cleared.llm_profiles[1].api_key.clear();
        sync_keys(&mut cleared, &changed, &store);
        assert_eq!(cleared.llm_profiles[1].api_key_storage, KeyStorage::None);
        assert!(store.entries.borrow().is_empty());
    }

    /// Deleting a profile deletes its key.
    #[test]
    fn deleting_a_profile_deletes_its_key() {
        let store = FakeStore::default();
        let mut prev = with_profiles(vec![LlmProfile::default(), work("sk-1")]);
        sync_keys(&mut prev, &with_profiles(vec![LlmProfile::default()]), &store);
        assert_eq!(store.entries.borrow().len(), 1);
        let mut next = with_profiles(vec![LlmProfile::default()]);
        sync_keys(&mut next, &prev, &store);
        assert!(store.entries.borrow().is_empty());
    }

    /// The UI can't claim a key is in the store to keep it out of the file.
    #[test]
    fn sync_ignores_the_storage_the_ui_sends() {
        let store = FakeStore::default();
        store.broken.set(true);
        let prev = with_profiles(vec![LlmProfile::default()]);
        let mut p = work("sk-1");
        p.api_key_storage = KeyStorage::Keychain;
        let mut next = with_profiles(vec![LlmProfile::default(), p]);
        sync_keys(&mut next, &prev, &store);
        assert_eq!(next.llm_profiles[1].api_key_storage, KeyStorage::File);
        assert!(serde_json::to_string(&next.for_disk()).unwrap().contains("sk-1"));
    }

    /// A key typed while no store works is saved in the file; the entry of
    /// the key it replaces is removed so it can't come back.
    #[test]
    fn sync_falls_back_to_the_file_when_the_store_fails() {
        let store = FakeStore::default();
        let mut prev = with_profiles(vec![work("sk-1")]);
        sync_keys(&mut prev, &with_profiles(vec![]), &store);
        assert_eq!(prev.llm_profiles[0].api_key_storage, KeyStorage::Keychain);
        // The store fails writes only from now on.
        struct WriteFails<'a>(&'a FakeStore);
        impl SecretStore for WriteFails<'_> {
            fn get(&self, a: &str) -> Result<Option<String>, StoreError> {
                self.0.get(a)
            }
            fn set(&self, _: &str, _: &str) -> Result<(), StoreError> {
                Err(StoreError("denied".into()))
            }
            fn delete(&self, a: &str) -> Result<(), StoreError> {
                self.0.delete(a)
            }
        }
        let mut next = prev.clone();
        next.llm_profiles[0].api_key = "sk-2".into();
        sync_keys(&mut next, &prev, &WriteFails(&store));
        assert_eq!(next.llm_profiles[0].api_key_storage, KeyStorage::File);
        assert!(store.entries.borrow().is_empty());
        assert!(serde_json::to_string(&next.for_disk()).unwrap().contains("sk-2"));
    }

    #[test]
    fn diagnostics_helpers_never_show_secrets() {
        for (url, want) in [
            ("https://api.example.com/v1", "https://api.example.com/v1"),
            ("http://localhost:11434", "http://localhost:11434"),
            ("https://user:sk-pass@api.example.com/v1", "https://…@api.example.com/v1"),
            ("https://api.example.com/v1?key=sk-q#frag", "https://api.example.com/v1?…"),
            ("user:sk@host:8080", "…@host:8080"),
        ] {
            assert_eq!(redact_url(url), want, "{url}");
        }
        let mut p = work("sk-1");
        for storage in [KeyStorage::None, KeyStorage::Keychain, KeyStorage::File, KeyStorage::Unreadable] {
            p.api_key_storage = storage;
            assert!(!key_summary(&p).contains("sk-1"));
        }
        assert_eq!(key_summary(&LlmProfile::default()), "none");
    }

    #[test]
    fn status_reports_the_store() {
        let store = FakeStore::default();
        assert!(status(&store).available);
        store.broken.set(true);
        let st = status(&store);
        assert!(!st.available);
        assert_eq!(st.error, "unavailable");
    }

    /// The real OS store, end to end. Touches the user's keychain (and may
    /// prompt), so it only runs on request:
    /// `cargo test real_credential_store -- --ignored`.
    #[test]
    #[ignore]
    fn real_credential_store_roundtrip() {
        let acct = account("sussurro-test-159");
        OsStore.set(&acct, "sk-test-value").expect("write");
        assert_eq!(OsStore.get(&acct).expect("read").as_deref(), Some("sk-test-value"));
        OsStore.delete(&acct).expect("delete");
        assert_eq!(OsStore.get(&acct).expect("read after delete"), None);
        OsStore.delete(&acct).expect("deleting a missing entry is fine");
        assert!(probe(&OsStore).is_ok());
    }
}
