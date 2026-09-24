//! Where the bundled `llama-server` sidecar lives (plan E9, #116).
//!
//! This module finds the files; [`super::remote`] starts and stops them
//! (#117). The sidecar is a pinned upstream llama.cpp build
//! (`sidecar/llama-server.lock.json`), fetched and SHA-256-verified by
//! `npm run sidecar` and merged into bundling builds by
//! `tauri.sidecar.conf.json`. A build without that config simply has no
//! sidecar, and [`sidecar_available`] says so.
//!
//! Layout, identical in `tauri dev` (under `target/<profile>/`) and in the
//! installed app:
//! - the executable, Tauri `externalBin`, next to the app's own executable:
//!   `Contents/MacOS/` on macOS, the install folder on Windows, `/usr/bin`
//!   (deb/rpm) or `usr/bin` (AppImage) on Linux;
//! - its shared libraries (llama, mtmd, ggml and the ggml backends), a Tauri
//!   resource folder [`LIB_DIR`] in the resource directory.
//!
//! The upstream binaries are shipped byte-for-byte, so they only look for
//! their libraries next to themselves. Whoever spawns the sidecar must set:
//! - the working directory to the lib folder: ggml loads its backends
//!   (`ggml-cpu-*`, `ggml-vulkan`) from the executable's folder or the
//!   current directory;
//! - the platform library path to the lib folder, prepended:
//!   `DYLD_LIBRARY_PATH` (macOS; honoured because the upstream binary has no
//!   hardened runtime — revisit if Developer ID signing ever lands), `PATH`
//!   (Windows) or `LD_LIBRARY_PATH` (Linux).

use std::path::{Path, PathBuf};

/// Base name of the sidecar executable (Tauri strips the target triple).
/// Prefixed so a distro's own `/usr/bin/llama-server` never clashes.
pub const SIDECAR_NAME: &str = "sussurro-llama-server";

/// Resource folder with the sidecar's shared libraries.
pub const LIB_DIR: &str = "llama-server-libs";

/// The sidecar executable's file name on this platform.
pub fn binary_file_name() -> String {
    format!("{SIDECAR_NAME}{}", std::env::consts::EXE_SUFFIX)
}

/// Where the sidecar's files are, once both were found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarPaths {
    pub binary: PathBuf,
    pub lib_dir: PathBuf,
}

/// Finds the sidecar next to the app executable (`exe_dir`) and its
/// libraries in the resource directory. `None` when either is missing.
pub fn resolve(exe_dir: &Path, resource_dir: &Path) -> Option<SidecarPaths> {
    let binary = exe_dir.join(binary_file_name());
    let lib_dir = resource_dir.join(LIB_DIR);
    (binary.is_file() && lib_dir.is_dir()).then_some(SidecarPaths { binary, lib_dir })
}

/// The sidecar's paths in the running app: bundled, else (debug builds)
/// the checkout's `npm run sidecar` output ([`dev_paths`]).
pub fn locate<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<SidecarPaths> {
    bundled(app).or_else(dev_paths)
}

fn bundled<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<SidecarPaths> {
    use tauri::Manager;
    let exe = std::env::current_exe().ok()?;
    let resource_dir = app.path().resource_dir().ok()?;
    resolve(exe.parent()?, &resource_dir)
}

/// `npm run sidecar`'s output in `src-tauri/binaries/`, still carrying the
/// target triple: a plain `tauri dev` (without the sidecar `--config`)
/// then runs Qwen3-ASR too. Debug builds only — a release build never
/// looks into the source checkout.
pub fn dev_paths() -> Option<SidecarPaths> {
    if !cfg!(debug_assertions) {
        return None;
    }
    dev_resolve(&Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries"))
}

fn dev_resolve(binaries: &Path) -> Option<SidecarPaths> {
    let binary = binaries.join(format!(
        "{SIDECAR_NAME}-{}{}",
        env!("SUSSURRO_TARGET_TRIPLE"),
        std::env::consts::EXE_SUFFIX
    ));
    let lib_dir = binaries.join(LIB_DIR);
    (binary.is_file() && lib_dir.is_dir()).then_some(SidecarPaths { binary, lib_dir })
}

/// Whether this build ships the `llama-server` sidecar.
pub fn sidecar_available<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    locate(app).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_only_when_binary_and_libs_are_present() {
        let tmp = tempfile::tempdir().unwrap();
        let exe_dir = tmp.path().join("bin");
        let res_dir = tmp.path().join("res");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(&res_dir).unwrap();
        assert_eq!(resolve(&exe_dir, &res_dir), None);

        fs::write(exe_dir.join(binary_file_name()), b"bin").unwrap();
        assert_eq!(resolve(&exe_dir, &res_dir), None, "libs missing");

        fs::create_dir(res_dir.join(LIB_DIR)).unwrap();
        assert_eq!(
            resolve(&exe_dir, &res_dir),
            Some(SidecarPaths {
                binary: exe_dir.join(binary_file_name()),
                lib_dir: res_dir.join(LIB_DIR),
            })
        );
    }

    #[test]
    fn dev_checkout_layout_keeps_the_target_triple() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(dev_resolve(tmp.path()), None);
        let binary = tmp.path().join(format!(
            "{SIDECAR_NAME}-{}{}",
            env!("SUSSURRO_TARGET_TRIPLE"),
            std::env::consts::EXE_SUFFIX
        ));
        fs::write(&binary, b"bin").unwrap();
        assert_eq!(dev_resolve(tmp.path()), None, "libs missing");
        fs::create_dir(tmp.path().join(LIB_DIR)).unwrap();
        assert_eq!(
            dev_resolve(tmp.path()),
            Some(SidecarPaths {
                binary,
                lib_dir: tmp.path().join(LIB_DIR),
            })
        );
    }

    #[test]
    fn a_directory_named_like_the_binary_is_not_the_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join(binary_file_name())).unwrap();
        fs::create_dir(tmp.path().join(LIB_DIR)).unwrap();
        assert_eq!(resolve(tmp.path(), tmp.path()), None);
    }

    #[test]
    fn names_match_the_bundle_config() {
        // tauri.sidecar.conf.json: externalBin + resources must agree with
        // the constants the app looks for.
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.sidecar.conf.json")).unwrap();
        assert_eq!(
            conf["bundle"]["externalBin"][0],
            format!("binaries/{SIDECAR_NAME}")
        );
        assert_eq!(
            conf["bundle"]["resources"][format!("binaries/{LIB_DIR}/")],
            format!("{LIB_DIR}/")
        );
        assert!(binary_file_name().starts_with(SIDECAR_NAME));
    }
}
