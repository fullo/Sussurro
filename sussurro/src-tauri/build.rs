fn main() {
    // Windows: tauri-build embeds its Common Controls v6 manifest (needed by
    // the dialog plugin's `TaskDialogIndirect`) into the app binary only, so
    // `cargo test` binaries died at load with STATUS_ENTRYPOINT_NOT_FOUND.
    // Embed the same manifest through the linker instead, which reaches
    // every linked target — app, tests and examples alike.
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let mut windows = tauri_build::WindowsAttributes::new();
    if msvc {
        windows = tauri_build::WindowsAttributes::new_without_app_manifest();
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}
