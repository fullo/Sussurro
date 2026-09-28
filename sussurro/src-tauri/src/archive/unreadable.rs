//! "The archive folder can't be read" (#328).
//!
//! A scan of an archive root that is missing, not a folder or refused by
//! the OS (macOS privacy settings, permissions, an unmounted drive) is an
//! error, never "no items": the index and the voice profiles are derived
//! from the scan, and an empty answer would make them drop everything.
//! Folders deeper down that can't be read are skipped and reported by the
//! scan ([`super::store::ArchiveScan::unreadable`]) so what lies under
//! them is kept, not treated as deleted.
//!
//! The UI recognises the error by its `code` ([`ui_error`]).

use serde::Serialize;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Why the archive root can't be listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnreadableKind {
    /// Nothing at that path (moved, renamed, a drive not mounted).
    Missing,
    /// Something that is not a folder.
    NotADirectory,
    /// The OS refused (EACCES/EPERM: permissions, macOS privacy settings).
    PermissionDenied,
    /// Any other OS error.
    Other,
}

/// The archive root can't be listed. Carries the folder and the OS reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveUnreadable {
    pub path: PathBuf,
    pub kind: UnreadableKind,
    /// The OS's own words ("Operation not permitted (os error 1)").
    pub reason: String,
}

impl ArchiveUnreadable {
    pub fn from_io(path: &Path, e: &std::io::Error) -> Self {
        let kind = match e.kind() {
            ErrorKind::NotFound => UnreadableKind::Missing,
            ErrorKind::NotADirectory => UnreadableKind::NotADirectory,
            // Rust maps both EACCES and EPERM (macOS TCC) here.
            ErrorKind::PermissionDenied => UnreadableKind::PermissionDenied,
            _ => UnreadableKind::Other,
        };
        ArchiveUnreadable {
            path: path.to_path_buf(),
            kind,
            reason: e.to_string(),
        }
    }

    pub fn not_a_directory(path: &Path) -> Self {
        ArchiveUnreadable {
            path: path.to_path_buf(),
            kind: UnreadableKind::NotADirectory,
            reason: "it is not a folder".to_string(),
        }
    }

    pub fn is_permission(&self) -> bool {
        self.kind == UnreadableKind::PermissionDenied
    }
}

impl fmt::Display for ArchiveUnreadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the archive folder {} can't be read: {}",
            self.path.display(),
            self.reason
        )
    }
}

impl std::error::Error for ArchiveUnreadable {}

/// The [`ArchiveUnreadable`] somewhere in `e`'s chain, if any.
pub fn find(e: &anyhow::Error) -> Option<&ArchiveUnreadable> {
    e.chain()
        .find_map(|c| c.downcast_ref::<ArchiveUnreadable>())
}

/// What the UI receives for an unreadable archive.
#[derive(Serialize)]
struct UiError<'a> {
    code: &'static str,
    path: String,
    kind: UnreadableKind,
    permission: bool,
    reason: &'a str,
    message: String,
}

/// Stable code the frontend matches (`src/lib/archiveError.ts`).
pub const CODE: &str = "archive_unreadable";

/// A command error as the UI gets it: a JSON object with
/// `code: "archive_unreadable"` when the archive root can't be read, else
/// the error's text as before.
pub fn ui_error(e: &anyhow::Error) -> String {
    match find(e) {
        Some(u) => serde_json::to_string(&UiError {
            code: CODE,
            path: u.path.display().to_string(),
            kind: u.kind,
            permission: u.is_permission(),
            reason: &u.reason,
            message: u.to_string(),
        })
        .unwrap_or_else(|_| u.to_string()),
        None => format!("{e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn kinds_follow_the_os_error() {
        let p = Path::new("/somewhere");
        let of = |k: ErrorKind| ArchiveUnreadable::from_io(p, &std::io::Error::from(k)).kind;
        assert_eq!(of(ErrorKind::NotFound), UnreadableKind::Missing);
        assert_eq!(
            of(ErrorKind::PermissionDenied),
            UnreadableKind::PermissionDenied
        );
        assert_eq!(of(ErrorKind::Interrupted), UnreadableKind::Other);
        #[cfg(unix)]
        {
            // EPERM (what macOS privacy settings return) is a permission error too.
            let eperm = std::io::Error::from_raw_os_error(1);
            assert!(ArchiveUnreadable::from_io(p, &eperm).is_permission());
        }
    }

    #[test]
    fn ui_error_is_coded_json_through_context() {
        let u = ArchiveUnreadable::from_io(
            Path::new("/a/b"),
            &std::io::Error::from(ErrorKind::PermissionDenied),
        );
        let e = Err::<(), _>(u).context("syncing the index").unwrap_err();
        let v: serde_json::Value = serde_json::from_str(&ui_error(&e)).unwrap();
        assert_eq!(v["code"], CODE);
        assert_eq!(v["path"], Path::new("/a/b").display().to_string());
        assert_eq!(v["kind"], "permission_denied");
        assert_eq!(v["permission"], true);
        assert!(v["message"].as_str().unwrap().contains("can't be read"));

        let plain = anyhow::anyhow!("something else");
        assert_eq!(ui_error(&plain), "something else");
    }
}
