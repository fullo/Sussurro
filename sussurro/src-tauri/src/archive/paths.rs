//! Archive location, item folder naming and path confinement.
//!
//! An item id is the item folder's path relative to the archive root, with
//! `/` separators on every OS (`2026/09/2026-09-24-weekly-sync`). Ids come
//! back from the frontend, so every id is validated before it touches the
//! filesystem and the resolved folder must stay inside the archive — the same
//! spirit as the model-path confinement of #89 (`stt/models.rs`).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

/// Folder name of the archive inside Documents (P4).
pub const ARCHIVE_FOLDER: &str = "Sussurro";
/// Maximum length of the slug part of an item folder name.
pub const MAX_SLUG_LEN: usize = 60;
/// Slug used when the title has no usable characters.
const FALLBACK_SLUG: &str = "untitled";

/// Where the archive lives. `custom` (the `archive_dir` setting) wins when
/// set; `~/` expands to `home`. Otherwise `<documents>/Sussurro`, falling back
/// to `<home>/Documents/Sussurro` when the OS documents dir is unknown (Linux
/// without XDG user dirs). A relative custom path is rejected: it would be
/// resolved against whatever the process cwd happens to be.
pub fn resolve_archive_dir(
    documents: Option<PathBuf>,
    home: Option<PathBuf>,
    custom: &str,
) -> Result<PathBuf> {
    let custom = custom.trim();
    if !custom.is_empty() {
        let expanded = match custom
            .strip_prefix("~/")
            .or_else(|| custom.strip_prefix("~\\"))
        {
            Some(rest) => match &home {
                Some(h) => h.join(rest),
                None => bail!("cannot expand '~' in archive folder: home directory unknown"),
            },
            None if custom == "~" => match &home {
                Some(h) => h.clone(),
                None => bail!("cannot expand '~' in archive folder: home directory unknown"),
            },
            None => PathBuf::from(custom),
        };
        if !expanded.is_absolute() {
            bail!("archive folder must be an absolute path: '{custom}'");
        }
        return Ok(expanded);
    }
    if let Some(docs) = documents.filter(|d| !d.as_os_str().is_empty()) {
        return Ok(docs.join(ARCHIVE_FOLDER));
    }
    if let Some(h) = home.filter(|h| !h.as_os_str().is_empty()) {
        return Ok(h.join("Documents").join(ARCHIVE_FOLDER));
    }
    bail!("cannot locate the Documents folder — set an archive folder in Settings")
}

/// Create the archive folder and list it once (#115). On macOS the first
/// access to `~/Documents` shows the system's permission prompt: onboarding
/// calls this on purpose, so the prompt appears there and never when a
/// recording starts (P4). Elsewhere it just makes sure the folder exists.
pub fn prepare_archive_dir(dir: &Path) -> Result<()> {
    let denied = |e: &std::io::Error| e.kind() == std::io::ErrorKind::PermissionDenied;
    let hint = "allow Sussurro in System Settings → Privacy & Security → Files and Folders, or choose another folder";
    if let Err(e) = std::fs::create_dir_all(dir) {
        if denied(&e) && cfg!(target_os = "macos") {
            bail!(
                "cannot create {}: access was denied — {hint}",
                dir.display()
            );
        }
        return Err(e).with_context(|| format!("cannot create {}", dir.display()));
    }
    // The listing is what a denied Documents folder refuses even when the
    // folder already exists (created before, or by another app).
    if let Err(e) = std::fs::read_dir(dir) {
        if denied(&e) && cfg!(target_os = "macos") {
            bail!("cannot open {}: access was denied — {hint}", dir.display());
        }
        return Err(e).with_context(|| format!("cannot open {}", dir.display()));
    }
    Ok(())
}

/// Fold a character to ASCII where a sensible transliteration exists.
fn fold_char(c: char, out: &mut String) {
    match c {
        'ß' => out.push_str("ss"),
        'æ' | 'Æ' => out.push_str("ae"),
        'œ' | 'Œ' => out.push_str("oe"),
        'ø' | 'Ø' => out.push('o'),
        'đ' | 'Đ' | 'ð' | 'Ð' => out.push('d'),
        'ł' | 'Ł' => out.push('l'),
        'þ' | 'Þ' => out.push_str("th"),
        'ı' => out.push('i'),
        _ => {
            // NFD splits "è" into "e" + combining grave; keep the ASCII base.
            for d in c.nfd() {
                if d.is_ascii() {
                    out.push(d);
                }
            }
        }
    }
}

/// Folder-safe slug of a title: lowercase ASCII, accents folded (è → e),
/// every run of other characters collapsed to one `-`, trimmed, at most
/// [`MAX_SLUG_LEN`] characters (cut on a `-` boundary when possible).
pub fn slugify(title: &str) -> String {
    let mut folded = String::with_capacity(title.len());
    for c in title.chars() {
        fold_char(c, &mut folded);
    }
    let mut slug = String::with_capacity(folded.len());
    let mut dash = false;
    for c in folded.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !slug.is_empty() {
            slug.push('-');
            dash = true;
        }
    }
    let mut slug = slug.trim_end_matches('-').to_string();
    if slug.len() > MAX_SLUG_LEN {
        let cut = &slug[..MAX_SLUG_LEN];
        // Prefer cutting at a word boundary if it doesn't lose too much.
        let cut = match cut.rfind('-') {
            Some(i) if i >= MAX_SLUG_LEN / 2 => &cut[..i],
            _ => cut,
        };
        slug = cut.trim_end_matches('-').to_string();
    }
    if slug.is_empty() {
        FALLBACK_SLUG.to_string()
    } else {
        slug
    }
}

/// Folder name for the `n`-th candidate (1 = no suffix): `YYYY-MM-DD-slug`,
/// then `-2`, `-3`… on collision.
pub fn folder_name(date: chrono::NaiveDate, slug: &str, n: u32) -> String {
    let base = format!("{}-{slug}", date.format("%Y-%m-%d"));
    if n <= 1 {
        base
    } else {
        format!("{base}-{n}")
    }
}

/// Parent folder (relative, `/`-separated) for an item dated `date`: `YYYY/MM`.
pub fn month_dir(date: chrono::NaiveDate) -> String {
    date.format("%Y/%m").to_string()
}

/// Validate an item id: a relative `/`-separated path whose components are
/// plain names — no empty parts, no `.`/`..`, no leading dot (dot-dirs are
/// app metadata, never items), no backslashes, drive letters or NULs.
pub fn validate_item_id(id: &str) -> Result<()> {
    let bad = |why: &str| -> Result<()> { bail!("invalid item id '{id}': {why}") };
    if id.is_empty() {
        return bad("empty");
    }
    if id.starts_with('/') || Path::new(id).is_absolute() {
        return bad("absolute path");
    }
    if id.contains(['\\', ':', '\0']) {
        return bad("forbidden character");
    }
    for part in id.split('/') {
        if part.is_empty() {
            return bad("empty path component");
        }
        if part.starts_with('.') {
            return bad("dot component");
        }
    }
    Ok(())
}

/// The folder of item `id` inside `archive`, refusing anything that resolves
/// outside it (including through symlinks, once the folder exists).
pub fn item_dir(archive: &Path, id: &str) -> Result<PathBuf> {
    validate_item_id(id)?;
    let mut path = archive.to_path_buf();
    for part in id.split('/') {
        path.push(part);
    }
    if path.exists() {
        let root = archive
            .canonicalize()
            .with_context(|| format!("resolving archive {}", archive.display()))?;
        let canon = path
            .canonicalize()
            .with_context(|| format!("resolving item {}", path.display()))?;
        if !canon.starts_with(&root) || canon == root {
            bail!("item id '{id}' resolves outside the archive");
        }
    }
    Ok(path)
}

/// Id (relative `/` path) of a folder found inside `archive`.
pub fn id_from_dir(archive: &Path, dir: &Path) -> Option<String> {
    let rel = dir.strip_prefix(archive).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect::<Option<_>>()?;
    let id = parts.join("/");
    validate_item_id(&id).ok()?;
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(p: &str) -> PathBuf {
        // An absolute path on the current OS.
        if cfg!(windows) {
            PathBuf::from(format!(
                "C:\\{}",
                p.trim_start_matches('/').replace('/', "\\")
            ))
        } else {
            PathBuf::from(p)
        }
    }

    #[test]
    fn resolve_prefers_custom_then_documents_then_home() {
        let docs = Some(abs("/u/Docs"));
        let home = Some(abs("/u"));
        assert_eq!(
            resolve_archive_dir(docs.clone(), home.clone(), "").unwrap(),
            abs("/u/Docs").join("Sussurro")
        );
        assert_eq!(
            resolve_archive_dir(None, home.clone(), "  ").unwrap(),
            abs("/u").join("Documents").join("Sussurro")
        );
        let custom = abs("/vault/Transcripts");
        assert_eq!(
            resolve_archive_dir(docs, home, custom.to_str().unwrap()).unwrap(),
            custom
        );
    }

    #[test]
    fn resolve_expands_tilde_and_rejects_relative() {
        let home = Some(abs("/u"));
        assert_eq!(
            resolve_archive_dir(None, home.clone(), "~/Notes/Sussurro").unwrap(),
            abs("/u").join("Notes/Sussurro")
        );
        assert!(resolve_archive_dir(None, None, "~/x").is_err());
        assert!(resolve_archive_dir(None, home, "relative/dir").is_err());
    }

    #[test]
    fn resolve_fails_without_any_base() {
        assert!(resolve_archive_dir(None, None, "").is_err());
        assert!(resolve_archive_dir(Some(PathBuf::new()), None, "").is_err());
    }

    #[test]
    fn prepare_creates_the_folder_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Documents").join(ARCHIVE_FOLDER);
        prepare_archive_dir(&dir).unwrap();
        assert!(dir.is_dir());
        std::fs::write(dir.join("keep.md"), "x").unwrap();
        prepare_archive_dir(&dir).unwrap();
        assert!(
            dir.join("keep.md").is_file(),
            "an existing archive is left alone"
        );
    }

    #[test]
    fn prepare_fails_when_a_file_is_in_the_way() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(ARCHIVE_FOLDER);
        std::fs::write(&dir, "not a folder").unwrap();
        let err = prepare_archive_dir(&dir).unwrap_err();
        assert!(format!("{err:#}").contains(ARCHIVE_FOLDER), "{err:#}");
    }

    #[test]
    fn slugify_folds_accents_and_collapses_separators() {
        assert_eq!(
            slugify("Weekly sync — release 0.7"),
            "weekly-sync-release-0-7"
        );
        assert_eq!(
            slugify("Perché è già così? Straße"),
            "perche-e-gia-cosi-strasse"
        );
        assert_eq!(slugify("  --Ciao!!  "), "ciao");
        assert_eq!(slugify("会议"), "untitled");
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("../../etc/passwd"), "etc-passwd");
    }

    #[test]
    fn slugify_caps_length_on_word_boundary() {
        let long = "parola ".repeat(30);
        let s = slugify(&long);
        assert!(s.len() <= MAX_SLUG_LEN, "{s}");
        assert!(s.ends_with("parola"));
        let one_word = "a".repeat(200);
        assert_eq!(slugify(&one_word).len(), MAX_SLUG_LEN);
    }

    #[test]
    fn folder_names_and_month_dir() {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 9, 4).unwrap();
        assert_eq!(folder_name(d, "sync", 1), "2026-09-04-sync");
        assert_eq!(folder_name(d, "sync", 3), "2026-09-04-sync-3");
        assert_eq!(month_dir(d), "2026/09");
    }

    #[test]
    fn validate_item_id_rejects_traversal_and_absolute() {
        assert!(validate_item_id("2026/09/2026-09-24-sync").is_ok());
        for bad in [
            "",
            "/etc",
            "../x",
            "2026/../../x",
            "2026//x",
            "2026/./x",
            ".sussurro",
            "2026/.hidden",
            "C:\\x",
            "c:/x",
            "a\\b",
            "a\0b",
            "2026/09/",
        ] {
            assert!(validate_item_id(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn item_dir_is_confined_to_the_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("arch");
        std::fs::create_dir_all(archive.join("2026/09/item")).unwrap();
        assert!(item_dir(&archive, "2026/09/item").is_ok());
        assert!(item_dir(&archive, "../arch/2026").is_err());
        // Not existing yet: still joined inside the archive.
        assert_eq!(
            item_dir(&archive, "2026/10/new").unwrap(),
            archive.join("2026").join("10").join("new")
        );
    }

    #[cfg(unix)]
    #[test]
    fn item_dir_rejects_symlink_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("arch");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, archive.join("link")).unwrap();
        assert!(item_dir(&archive, "link").is_err());
    }

    #[test]
    fn id_from_dir_uses_forward_slashes() {
        let archive = abs("/a");
        let dir = archive.join("2026").join("09").join("x");
        assert_eq!(id_from_dir(&archive, &dir).as_deref(), Some("2026/09/x"));
        assert_eq!(id_from_dir(&archive, &abs("/elsewhere")), None);
    }
}
