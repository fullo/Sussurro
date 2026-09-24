use super::*;
use crate::cleanup::ollama::{chat_bundled, ChatOptions};
use crate::engine::checkpoint::process_alive;
use crate::stt::remote::tests::{fake_chat_config, fake_config, serial, wait_dead};
use crate::stt::remote::{kill_all, RemoteTranscriber};
use serde_json::json;
use std::sync::Arc;

// --------------------------------------------------------------- profile --

#[test]
fn the_profile_is_local_openai_compatible_and_marked_bundled() {
    let p = profile();
    assert_eq!(p.id, PROFILE_ID);
    assert_eq!(p.name, "Local (bundled)");
    assert_eq!(p.api, CleanupApi::Openai);
    assert!(p.bundled);
    assert!(!p.external, "never external");
    assert!(!crate::llm::infer_external(&p.base_url), "its stored address reads as local too");
    assert!(p.cleanup_allowed(), "no opt-in needed");
    assert!(p.api_key.is_empty());
    assert_eq!(p.model, MODEL_ID);
    assert_eq!(p.context_tokens, CONTEXT_TOKENS);
    assert_eq!(p.effective_context_tokens(), CONTEXT_TOKENS);
    assert_eq!(p.host(), "127.0.0.1");

    // `bundled` is stored for the built-in profile only.
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(json["bundled"], true);
    let back: LlmProfile = serde_json::from_value(json).unwrap();
    assert_eq!(back, p);
    let other = serde_json::to_value(LlmProfile::default()).unwrap();
    assert!(other.get("bundled").is_none());
    let old: LlmProfile = serde_json::from_str(r#"{"id":"x","external":false}"#).unwrap();
    assert!(!old.bundled);
}

#[test]
fn the_sidecar_config_serves_the_model_as_the_profile_model() {
    let paths = SidecarPaths {
        binary: "/app/sussurro-llama-server".into(),
        lib_dir: "/app/llama-server-libs".into(),
    };
    let cfg = sidecar_config(&paths, Path::new("/models"), None);
    assert_eq!(cfg.model, Path::new("/models").join(MODEL_FILE));
    assert_eq!(
        cfg.role,
        crate::stt::remote::SidecarRole::Chat {
            ctx_tokens: CONTEXT_TOKENS,
            alias: MODEL_ID.into()
        }
    );
    let args = cfg.server_args(1234);
    let pos = |a: &str| args.iter().position(|x| x == a).unwrap();
    assert_eq!(args[pos("--alias") + 1], profile().model.as_str());
    assert_eq!(args[pos("--host") + 1], "127.0.0.1");
}

#[test]
fn status_carries_the_download_size() {
    let s = BundledStatus::new(true, false, false);
    assert_eq!(s.download_bytes, MODEL_BYTES);
    assert_eq!(s.model, MODEL_LABEL);
    let json = serde_json::to_value(&s).unwrap();
    for k in ["available", "downloaded", "running", "model", "download_bytes"] {
        assert!(json.get(k).is_some(), "{k}");
    }
}

// ------------------------------------------------ lifecycle (fake server) --

fn fake_llm(dir: &Path, mode: &str) -> BundledLlm {
    let cfg = fake_chat_config(dir, "chat.gguf", mode);
    BundledLlm::new(move || Ok(cfg.clone()))
}

fn user(text: &str) -> Vec<serde_json::Value> {
    vec![json!({"role": "system", "content": "clean"}), json!({"role": "user", "content": text})]
}

#[test]
fn chat_starts_the_server_on_first_use_and_reuses_it() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let llm = fake_llm(dir.path(), "ok");
    assert!(!llm.is_running() && llm.pid().is_none(), "nothing runs before a request");

    let out = chat_bundled(&llm, &user("ciao"), &ChatOptions::default()).unwrap();
    assert_eq!(out, "ciao");
    let pid = llm.pid().unwrap();
    assert!(process_alive(pid));
    assert!(llm.port().is_some_and(|p| p > 0));

    // The next request goes to the same process.
    assert_eq!(chat_bundled(&llm, &user("hello"), &ChatOptions::default()).unwrap(), "hello");
    assert_eq!(llm.pid(), Some(pid));

    // Stopped with its owner.
    drop(llm);
    assert!(wait_dead(pid));
}

#[test]
fn idle_stop_waits_for_the_threshold_and_for_requests_in_flight() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let llm = Arc::new(fake_llm(dir.path(), "slow"));
    assert!(!llm.stop_if_idle(Duration::ZERO), "nothing to stop yet");
    llm.warm().unwrap();
    let pid = llm.pid().unwrap();
    assert!(!llm.stop_if_idle(Duration::from_secs(3600)), "used just now");

    // A request in flight (the fake takes 3 s) is never idle.
    let busy = {
        let llm = llm.clone();
        std::thread::spawn(move || chat_bundled(&llm, &user("slow"), &ChatOptions::default()))
    };
    let until = Instant::now() + Duration::from_secs(2);
    let mut kept = true;
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
        kept &= !llm.stop_if_idle(Duration::ZERO);
    }
    assert!(kept, "stopped during a request");
    assert_eq!(busy.join().unwrap().unwrap(), "slow");
    assert!(process_alive(pid));

    // Idle: stopped, and the next request starts a new process.
    assert!(llm.stop_if_idle(Duration::ZERO));
    assert!(wait_dead(pid));
    assert!(!llm.is_running());
    llm.warm().unwrap();
    assert!(llm.pid().is_some_and(|p| p != pid));
    assert!(llm.stop());
}

#[test]
fn a_crash_during_a_request_restarts_the_server_and_retries_once() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let llm = fake_llm(dir.path(), "crash_once");
    llm.warm().unwrap();
    let first = llm.pid().unwrap();
    let out = chat_bundled(&llm, &user("again"), &ChatOptions::default()).unwrap();
    assert_eq!(out, "again");
    let second = llm.pid().unwrap();
    assert_ne!(first, second);
    assert!(wait_dead(first));

    // Killed between requests (app exit hook, OS): restarted on next use.
    kill_all();
    assert!(wait_dead(second));
    assert_eq!(chat_bundled(&llm, &user("x"), &ChatOptions::default()).unwrap(), "x");
    assert!(llm.pid().is_some_and(|p| p != second));
}

#[test]
fn a_server_that_always_crashes_fails_the_request() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let llm = fake_llm(dir.path(), "crash_always");
    let err = chat_bundled(&llm, &user("x"), &ChatOptions::default()).unwrap_err();
    assert!(format!("{err:#}").contains("crashed twice"), "{err:#}");
    assert!(!llm.is_running());
}

#[test]
fn an_error_from_a_live_server_keeps_it() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let llm = fake_llm(dir.path(), "ok");
    llm.warm().unwrap();
    let pid = llm.pid().unwrap();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let err = llm
        .with_server(|_| -> Result<()> {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(anyhow!("model said no"))
        })
        .unwrap_err();
    assert_eq!(err.to_string(), "model said no");
    assert_eq!(calls.into_inner(), 1, "no retry on a live server");
    assert_eq!(llm.pid(), Some(pid));
}

#[test]
fn a_missing_model_or_sidecar_fails_without_starting_anything() {
    let _serial = serial();
    let llm = BundledLlm::new(|| Err(anyhow!(NOT_DOWNLOADED)));
    let err = chat_bundled(&llm, &user("x"), &ChatOptions::default()).unwrap_err();
    assert!(err.to_string().contains("not downloaded"), "{err}");
    assert!(llm.pid().is_none());
    assert!(llm.warm().is_err());
    // The app's instance, with no app running (unit tests): no sidecar.
    assert!(list_models().is_err());
}

#[test]
fn another_models_folder_restarts_the_server() {
    let _serial = serial();
    let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = fake_chat_config(d1.path(), "chat.gguf", "ok");
    let b = fake_chat_config(d2.path(), "chat.gguf", "ok");
    let current = Arc::new(Mutex::new(a));
    let llm = {
        let current = current.clone();
        BundledLlm::new(move || Ok(current.lock().unwrap().clone()))
    };
    llm.warm().unwrap();
    let first = llm.pid().unwrap();
    llm.warm().unwrap();
    assert_eq!(llm.pid(), Some(first), "same config: same process");
    *current.lock().unwrap() = b;
    llm.warm().unwrap();
    assert!(wait_dead(first), "the old folder's server stopped");
    assert!(llm.pid().is_some_and(|p| p != first));
}

/// STT and LLM run as two separate `llama-server` processes (see the module
/// docs): each stops on its own, and the exit hook stops both.
#[test]
fn it_runs_beside_the_qwen3_asr_sidecar_with_its_own_lifecycle() {
    let _serial = serial();
    let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let asr = RemoteTranscriber::start(fake_config(d1.path(), "ok")).unwrap();
    let asr_pid = asr.sidecar().pid().unwrap();
    let asr_port = asr.sidecar().port();
    let llm = fake_llm(d2.path(), "ok");
    assert_eq!(chat_bundled(&llm, &user("a"), &ChatOptions::default()).unwrap(), "a");
    let llm_pid = llm.pid().unwrap();
    assert_ne!(asr_pid, llm_pid, "two processes");
    assert_ne!(Some(asr_port), llm.port(), "two ports");

    // The transcriber's idle unload stops only Qwen3-ASR.
    let slot = Mutex::new(Some(asr));
    let last = Mutex::new(Instant::now().checked_sub(Duration::from_secs(10)));
    assert!(crate::pipeline::unload_if_idle(&slot, &last, Duration::from_secs(1), false));
    assert!(wait_dead(asr_pid));
    assert!(process_alive(llm_pid));
    assert_eq!(chat_bundled(&llm, &user("b"), &ChatOptions::default()).unwrap(), "b");
    assert_eq!(llm.pid(), Some(llm_pid));

    // The LLM's idle stop leaves a (new) Qwen3-ASR process alone.
    let asr = RemoteTranscriber::start(fake_config(d1.path(), "ok")).unwrap();
    let asr_pid = asr.sidecar().pid().unwrap();
    assert!(llm.stop_if_idle(Duration::ZERO));
    assert!(wait_dead(llm_pid));
    assert!(process_alive(asr_pid));

    // App exit: every sidecar goes.
    llm.warm().unwrap();
    let llm_pid = llm.pid().unwrap();
    assert_eq!(kill_all(), 2);
    assert!(wait_dead(asr_pid) && wait_dead(llm_pid));
    drop(asr);
}

// ----------------------------------------------------------------- live --

/// The real sidecar and model clean an Italian and an English dictation
/// with Sussurro's own cleanup prompt. Nothing is downloaded; point the
/// variables at existing files:
///
/// ```sh
/// SUSSURRO_TEST_LLAMA_SERVER=…/llama-b11146/llama-server \
/// SUSSURRO_TEST_BUNDLED_LLM=…/Qwen3-1.7B-Q4_K_M.gguf \
/// cargo test live_bundled_llm -- --ignored --nocapture
/// ```
///
/// `SUSSURRO_TEST_LLAMA_LIBS` overrides the libs folder (default: the
/// binary's own folder, the upstream archive layout).
#[test]
#[ignore]
fn live_bundled_llm_cleans_italian_and_english() {
    use crate::settings::{CleanupLevel, Settings};
    let _serial = serial();
    let binary = PathBuf::from(
        std::env::var("SUSSURRO_TEST_LLAMA_SERVER").expect("set SUSSURRO_TEST_LLAMA_SERVER"),
    );
    let libs = std::env::var("SUSSURRO_TEST_LLAMA_LIBS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| binary.parent().unwrap().to_path_buf());
    let model = PathBuf::from(
        std::env::var("SUSSURRO_TEST_BUNDLED_LLM").expect("set SUSSURRO_TEST_BUNDLED_LLM"),
    );
    let log = tempfile::tempdir().unwrap();
    let paths = SidecarPaths { binary, lib_dir: libs };
    let mut cfg = sidecar_config(&paths, model.parent().unwrap(), Some(log.path().join("llm.log")));
    cfg.model = model;
    let llm = BundledLlm::new(move || Ok(cfg.clone()));
    let started = Instant::now();
    llm.warm().unwrap();
    println!("healthy after {:.1} s", started.elapsed().as_secs_f32());
    let pid = llm.pid().unwrap();

    let settings = Settings {
        cleanup_level: CleanupLevel::Light,
        llm_profiles: vec![profile()],
        cleanup_profile: PROFILE_ID.into(),
        voice_commands: false,
        ..Default::default()
    };
    // What must go: a repeated word in both, the English fillers the Light
    // prompt names. (Italian "ehm" is not in the prompt's filler list and
    // the 1.7B model keeps it about half the time: not asserted.)
    for (raw, must_keep, must_go, other_language) in [
        (
            "uhm volevo dire che il il documento è pronto ma manca ancora la parte sui costi",
            ["documento", "pronto", "costi"],
            &[" il il "][..],
            " the ",
        ),
        (
            "um so basically uh we need to send the the quote to the client tomorrow and then uh call mark about the meeting",
            ["quote", "client", "meeting"],
            &[" um ", " uh ", " the the "][..],
            " il ",
        ),
    ] {
        let messages = crate::cleanup::prompt::build_messages(&settings, None, raw).unwrap();
        let started = Instant::now();
        let out = chat_bundled(&llm, &messages, &ChatOptions::default()).unwrap();
        println!("{:.2} s: {raw}\n  → {out}", started.elapsed().as_secs_f32());
        let lower = format!(" {} ", out.to_lowercase());
        for w in must_keep {
            assert!(lower.contains(w), "lost “{w}”: {out}");
        }
        for &f in must_go {
            assert!(!lower.contains(f), "kept “{f}”: {out}");
        }
        assert!(out.starts_with(char::is_uppercase), "not cleaned: {out}");
        assert!(!lower.contains(other_language), "translated: {out}");
        assert!(!out.contains("<think>"), "reasoning is off: {out}");
        assert!(
            !crate::cleanup::prompt::looks_hallucinated(&settings.cleanup_level, raw, &out),
            "the guard would drop it: {out}"
        );
    }
    drop(llm);
    assert!(wait_dead(pid));
}
