fn main() {
    // `stt::sidecar::dev_paths` finds `npm run sidecar`'s output, which is
    // named after the target triple (Tauri's externalBin naming).
    println!(
        "cargo:rustc-env=SUSSURRO_TARGET_TRIPLE={}",
        std::env::var("TARGET").expect("cargo sets TARGET for build scripts")
    );
    tauri_build::build()
}
