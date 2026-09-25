use super::*;
use crate::engine::checkpoint::process_alive;

// ------------------------------------------------------------- sanitiser --

#[test]
fn sanitiser_strips_the_language_prefix_and_reads_the_language() {
    let out = sanitize("language Italian<asr_text>Le Sundarbans sono state dichiarate patrimonio.");
    assert_eq!(out.text, "Le Sundarbans sono state dichiarate patrimonio.");
    assert_eq!(out.language.as_deref(), Some("it"));

    let out = sanitize("language English<asr_text>Ancient China had a unique way.");
    assert_eq!(out.text, "Ancient China had a unique way.");
    assert_eq!(out.language.as_deref(), Some("en"));
}

#[test]
fn sanitiser_passes_plain_text_through() {
    let out = sanitize("  just the words  ");
    assert_eq!(out.text, "just the words");
    assert_eq!(out.language, None);
    // A sentence that merely mentions a language is not a prefix.
    let out = sanitize("the language English is spoken here");
    assert_eq!(out.text, "the language English is spoken here");
}

#[test]
fn sanitiser_keeps_the_text_after_the_last_marker() {
    let out = sanitize("language English<asr_text>draft<asr_text>final words");
    assert_eq!(out.text, "final words");
    assert_eq!(
        out.language, None,
        "no language right before the last marker"
    );
    let out = sanitize("language French<asr_text>bonjour</asr_text>");
    assert_eq!(out.text, "bonjour");
    assert_eq!(out.language.as_deref(), Some("fr"));
}

#[test]
fn sanitiser_removes_special_tokens() {
    let out = sanitize("<|im_start|>language German<asr_text>Guten Tag<|im_end|><|endoftext|>");
    assert_eq!(out.text, "Guten Tag");
    assert_eq!(out.language.as_deref(), Some("de"));
    let out = sanitize("hello <|audio_pad|> world");
    assert_eq!(out.text, "hello world");
}

#[test]
fn sanitiser_handles_silence_and_unknown_languages() {
    for raw in [
        "language None<asr_text>",
        "language English",
        "language None",
        "",
        "   ",
    ] {
        assert_eq!(sanitize(raw).text, "", "{raw:?}");
    }
    assert_eq!(sanitize("language English").language.as_deref(), Some("en"));
    assert_eq!(sanitize("language None<asr_text>").language, None);
    let out = sanitize("language Klingon<asr_text>Qapla'");
    assert_eq!(out.text, "Qapla'");
    assert_eq!(out.language, None);
}

#[test]
fn sanitiser_collapses_whitespace() {
    assert_eq!(
        sanitize("language English<asr_text> a \n b\t c ").text,
        "a b c"
    );
}

#[test]
fn language_names_map_to_iso_codes() {
    assert_eq!(language_code("Italian"), Some("it"));
    assert_eq!(language_code("ENGLISH"), Some("en"));
    assert_eq!(language_code("Cantonese"), Some("zh"));
    assert_eq!(language_code("None"), None);
}

// ---------------------------------------------------------------- input --

fn tone(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 * ((i as f32) * 0.05).sin()).collect()
}

fn lens(r: &[std::ops::Range<usize>]) -> Vec<usize> {
    r.iter().map(|x| x.len()).collect()
}

fn assert_tiles(r: &[std::ops::Range<usize>], len: usize) {
    assert_eq!(r.first().unwrap().start, 0);
    assert_eq!(r.last().unwrap().end, len);
    assert!(r.windows(2).all(|w| w[0].end == w[1].start));
}

#[test]
fn input_up_to_30_s_is_one_piece() {
    for n in [0, 1_000, MAX_INPUT_SAMPLES] {
        let s = tone(n);
        assert_eq!(input_pieces(&s), vec![0..n], "{n}");
    }
    // An engine segment of 30 s with a pause in it is still sent whole.
    let mut s = tone(MAX_INPUT_SAMPLES);
    s[22 * 16_000..23 * 16_000]
        .iter_mut()
        .for_each(|x| *x = 0.0);
    assert_eq!(input_pieces(&s).len(), 1);
}

#[test]
fn long_input_is_cut_at_pauses_under_the_cap() {
    // 70 s of speech with 0.5 s pauses at 25 s and 50 s.
    let rate = 16_000;
    let mut s = tone(70 * rate);
    for at in [25 * rate, 50 * rate] {
        s[at..at + rate / 2].iter_mut().for_each(|x| *x = 0.0);
    }
    let r = input_pieces(&s);
    assert_tiles(&r, s.len());
    assert_eq!(r.len(), 3, "{:?}", lens(&r));
    assert!(r.iter().all(|p| p.len() <= MAX_INPUT_SAMPLES));
    // Each cut falls inside a pause.
    assert!(
        (25 * rate..25 * rate + rate / 2).contains(&r[0].end),
        "cut at {}",
        r[0].end
    );
    assert!(
        (50 * rate..50 * rate + rate / 2).contains(&r[1].end),
        "cut at {}",
        r[1].end
    );
}

#[test]
fn input_without_pauses_is_still_capped() {
    let s = tone(95 * 16_000);
    let r = input_pieces(&s);
    assert_tiles(&r, s.len());
    assert!(r.len() >= 4);
    assert!(
        r.iter()
            .all(|p| !p.is_empty() && p.len() <= MAX_INPUT_SAMPLES),
        "{:?}",
        lens(&r)
    );
}

#[test]
fn multipart_body_has_the_fields_and_the_file() {
    let wav = crate::archive::audio::wav_bytes(&tone(10));
    let body = multipart_body("B0UND", &[("model", "qwen3-asr")], "audio.wav", &wav);
    let text = String::from_utf8_lossy(&body);
    assert!(text.starts_with(
        "--B0UND\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nqwen3-asr\r\n"
    ));
    assert!(text.contains(
        "--B0UND\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF"
    ));
    assert!(text.ends_with("\r\n--B0UND--\r\n"));
    // The WAV bytes are in verbatim.
    assert!(body.windows(wav.len()).any(|w| w == wav.as_slice()));
    let b = new_boundary().unwrap();
    assert!(b.starts_with("sussurro-") && b.len() == "sussurro-".len() + 32);
    assert_ne!(b, new_boundary().unwrap());
}

// -------------------------------------------------------------- command --

#[test]
fn server_args_bind_loopback_only_and_keep_paths_whole() {
    let model = Path::new("/models dir/Qwen3 ASR.gguf");
    let mmproj = Path::new("/models dir/mmproj; rm -rf ~.gguf");
    let args = server_args(model, mmproj, 43_210);
    let pos = |a: &str| args.iter().position(|x| x == a).unwrap();
    assert_eq!(
        args[pos("-m") + 1],
        model.as_os_str(),
        "a path with spaces is one argument"
    );
    assert_eq!(
        args[pos("--mmproj") + 1],
        mmproj.as_os_str(),
        "shell syntax stays literal"
    );
    assert_eq!(args[pos("--host") + 1], "127.0.0.1");
    assert_eq!(args[pos("--port") + 1], "43210");
    for flag in ["-ngl", "-c", "-np", "--cache-ram", "--no-webui"] {
        assert!(args.iter().any(|a| a == flag), "{flag}");
    }
    assert!(!args.iter().any(|a| a == "0.0.0.0"));
}

#[test]
fn chat_server_args_bind_loopback_only_with_the_alias_and_no_reasoning() {
    let model = Path::new("/models dir/Qwen3 1.7B; echo.gguf");
    let args = chat_server_args(model, "qwen3-1.7b", 8192, 40_001);
    let pos = |a: &str| args.iter().position(|x| x == a).unwrap();
    assert_eq!(args[pos("-m") + 1], model.as_os_str(), "one argument, literal");
    assert_eq!(args[pos("--alias") + 1], "qwen3-1.7b");
    assert_eq!(args[pos("--host") + 1], "127.0.0.1");
    assert_eq!(args[pos("--port") + 1], "40001");
    assert_eq!(args[pos("-c") + 1], "8192");
    assert_eq!(args[pos("-np") + 1], "1");
    assert_eq!(args[pos("--reasoning") + 1], "off");
    assert!(args.iter().any(|a| a == "--no-webui"));
    assert!(!args.iter().any(|a| a == "--mmproj" || a == "0.0.0.0"));

    // The config picks the arguments by role.
    let chat = SidecarConfig::chat("/b".into(), "/l".into(), model.into(), 8192, "qwen3-1.7b");
    assert_eq!(chat.server_args(40_001), args);
    assert_eq!(chat.label(), "bundled LLM");
    let asr = SidecarConfig::new("/b".into(), "/l".into(), "/m.gguf".into(), "/p.gguf".into());
    assert_eq!(asr.role, SidecarRole::Asr);
    assert_eq!(asr.label(), "Qwen3-ASR");
    assert_eq!(
        asr.server_args(1),
        server_args(Path::new("/m.gguf"), Path::new("/p.gguf"), 1)
    );
}

#[test]
fn command_runs_the_binary_directly_from_the_lib_dir() {
    let mut cfg = SidecarConfig::new(
        "/app/sussurro-llama-server".into(),
        "/app/llama-server-libs".into(),
        "/m/model.gguf".into(),
        "/m/mmproj.gguf".into(),
    );
    cfg.prefix_args = vec!["--prefix".into()];
    let cmd = command(&cfg, 5_000);
    assert_eq!(cmd.get_program(), OsStr::new("/app/sussurro-llama-server"));
    let args: Vec<&OsStr> = cmd.get_args().collect();
    assert_eq!(args[0], "--prefix");
    assert_eq!(
        args[1..]
            .iter()
            .map(|a| a.to_os_string())
            .collect::<Vec<_>>(),
        server_args(&cfg.model, &cfg.mmproj, 5_000)
    );
    assert_eq!(
        cmd.get_current_dir(),
        Some(Path::new("/app/llama-server-libs"))
    );
    let var = library_path_var(std::env::consts::OS);
    let (_, value) = cmd
        .get_envs()
        .find(|(k, _)| *k == OsStr::new(var))
        .expect("library path set");
    let value = value.unwrap().to_string_lossy().into_owned();
    assert!(value.starts_with("/app/llama-server-libs"), "{value}");
}

#[test]
fn library_path_per_os() {
    assert_eq!(library_path_var("macos"), "DYLD_LIBRARY_PATH");
    assert_eq!(library_path_var("windows"), "PATH");
    assert_eq!(library_path_var("linux"), "LD_LIBRARY_PATH");
    let lib = Path::new("/libs");
    assert_eq!(library_path_value("linux", lib, None), "/libs");
    assert_eq!(
        library_path_value("linux", lib, Some(OsStr::new(""))),
        "/libs"
    );
    assert_eq!(
        library_path_value("linux", lib, Some(OsStr::new("/usr/lib"))),
        "/libs:/usr/lib"
    );
    assert_eq!(
        library_path_value("macos", lib, Some(OsStr::new("/opt/lib"))),
        "/libs:/opt/lib"
    );
    assert_eq!(
        library_path_value(
            "windows",
            Path::new(r"C:\App\libs"),
            Some(OsStr::new(r"C:\Windows;C:\Tools"))
        ),
        r"C:\App\libs;C:\Windows;C:\Tools"
    );
}

#[test]
fn backoff_doubles_and_caps() {
    let base = Duration::from_millis(500);
    // A first crash restarts at once: usually a one-off.
    assert_eq!(backoff(base, 0), Duration::ZERO);
    assert_eq!(backoff(base, 1), Duration::ZERO);
    assert_eq!(backoff(base, 2), Duration::from_millis(500));
    assert_eq!(backoff(base, 3), Duration::from_secs(1));
    assert_eq!(backoff(base, 5), Duration::from_secs(4));
    assert_eq!(backoff(base, 10), Duration::from_secs(60));
    assert_eq!(backoff(base, u32::MAX), Duration::from_secs(60));
}

// ------------------------------------------------ lifecycle (fake server) --

/// The lifecycle tests run the test binary itself as the sidecar: it is
/// started with the `prefix_args` below, so libtest runs only
/// [`fake_sidecar_server`], which finds `--port` among its arguments and
/// serves. The fake's behaviour is written in the "model" file.
pub(crate) fn fake_config(dir: &Path, mode: &str) -> SidecarConfig {
    let libs = dir.join("libs");
    std::fs::create_dir_all(&libs).unwrap();
    let model = dir.join("model.gguf");
    std::fs::write(&model, mode).unwrap();
    let mmproj = dir.join("mmproj.gguf");
    std::fs::write(&mmproj, "").unwrap();
    as_fake(SidecarConfig::new(std::env::current_exe().unwrap(), libs, model, mmproj), dir)
}

/// [`fake_config`] for a chat-model sidecar (#118): the fake answers
/// `/v1/chat/completions` and `/v1/models` too. `mode` as in
/// [`fake_sidecar_server`]; the model file is `name`, so two chat sidecars
/// can share a folder.
pub(crate) fn fake_chat_config(dir: &Path, name: &str, mode: &str) -> SidecarConfig {
    let libs = dir.join("libs");
    std::fs::create_dir_all(&libs).unwrap();
    let model = dir.join(name);
    std::fs::write(&model, mode).unwrap();
    as_fake(
        SidecarConfig::chat(std::env::current_exe().unwrap(), libs, model, 2048, "fake-chat"),
        dir,
    )
}

fn as_fake(mut cfg: SidecarConfig, dir: &Path) -> SidecarConfig {
    cfg.prefix_args = [
        "--exact",
        "stt::remote::tests::fake_sidecar_server",
        "--test-threads=1",
        "--",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    cfg.log_file = Some(dir.join("sidecar.log"));
    cfg.start_timeout = Duration::from_secs(30);
    cfg.request_timeout = Duration::from_secs(30);
    cfg.backoff_base = Duration::from_millis(10);
    cfg
}

/// The lifecycle tests share the process-wide registry (`kill_all`) —
/// also the bundled LLM's (`crate::llm::bundled`).
pub(crate) fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    lock(&LOCK)
}

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Not a test of its own: the fake `llama-server` (see [`fake_config`]).
/// Modes: `ok`; `crash_once` (dies mid-request the first time, then
/// serves); `crash_always`; `exit_now`; `never_healthy`.
#[test]
fn fake_sidecar_server() {
    let args: Vec<String> = std::env::args().collect();
    let Some(port) = arg_after(&args, "--port") else {
        return; // an ordinary test run
    };
    if arg_after(&args, "--host").as_deref() != Some("127.0.0.1") {
        std::process::exit(9);
    }
    let model = PathBuf::from(arg_after(&args, "-m").unwrap());
    let mode = std::fs::read_to_string(&model).unwrap_or_default();
    if mode == "exit_now" {
        std::process::exit(3);
    }
    let crash_marker = model.with_extension("crashed");
    let server = tiny_http::Server::http(format!("127.0.0.1:{port}")).unwrap();
    let started = Instant::now();
    let json = |v: serde_json::Value, code: u16| {
        tiny_http::Response::from_string(v.to_string()).with_status_code(code)
    };
    for mut req in server.incoming_requests() {
        let url = req.url().to_string();
        match (req.method(), url.as_str()) {
            (tiny_http::Method::Get, "/health") => {
                let ready =
                    mode != "never_healthy" && started.elapsed() > Duration::from_millis(200);
                let _ = if ready {
                    req.respond(json(serde_json::json!({"status": "ok"}), 200))
                } else {
                    req.respond(json(serde_json::json!({"error": "Loading model"}), 503))
                };
            }
            (tiny_http::Method::Get, "/env") => {
                let var = library_path_var(std::env::consts::OS);
                let _ = req.respond(json(
                    serde_json::json!({
                        "cwd": std::env::current_dir().unwrap(),
                        "lib": std::env::var(var).unwrap_or_default(),
                        "pid": std::process::id(),
                    }),
                    200,
                ));
            }
            (tiny_http::Method::Post, "/v1/audio/transcriptions") => {
                let mut body = Vec::new();
                let _ = req.as_reader().read_to_end(&mut body);
                let multipart = req.headers().iter().any(|h| {
                    h.field.equiv("Content-Type")
                        && h.value
                            .as_str()
                            .starts_with("multipart/form-data; boundary=")
                });
                // crash_once: the first instance leaves a marker and dies;
                // the restarted one finds it and serves.
                let crash = mode == "crash_always"
                    || (mode == "crash_once"
                        && !crash_marker.exists()
                        && std::fs::write(&crash_marker, "x").is_ok());
                if crash {
                    std::process::exit(1); // no answer: the connection drops
                }
                let has_wav = body.windows(4).any(|w| w == b"RIFF");
                if !multipart || !has_wav {
                    let _ = req.respond(json(serde_json::json!({"error": "bad request"}), 400));
                    continue;
                }
                let _ = req.respond(json(
                    serde_json::json!({"text": format!("language English<asr_text>heard {} bytes", body.len())}),
                    200,
                ));
            }
            // The chat model of the bundled LLM profile (#118).
            (tiny_http::Method::Get, "/v1/models") => {
                let alias = arg_after(&args, "--alias").unwrap_or_default();
                let _ = req.respond(json(serde_json::json!({"data": [{"id": alias}]}), 200));
            }
            (tiny_http::Method::Post, "/v1/chat/completions") => {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let crash = mode == "crash_always"
                    || (mode == "crash_once"
                        && !crash_marker.exists()
                        && std::fs::write(&crash_marker, "x").is_ok());
                if crash {
                    std::process::exit(1);
                }
                if mode == "slow" {
                    std::thread::sleep(Duration::from_secs(3));
                }
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let last = v["messages"]
                    .as_array()
                    .and_then(|m| m.last())
                    .and_then(|m| m["content"].as_str())
                    .unwrap_or_default()
                    .to_string();
                let _ = req.respond(json(
                    serde_json::json!({
                        "model": v["model"],
                        "choices": [{"message": {"role": "assistant", "content": last}}],
                    }),
                    200,
                ));
            }
            _ => {
                let _ = req.respond(tiny_http::Response::empty(404));
            }
        }
    }
}

fn get_env(port: u16) -> serde_json::Value {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://127.0.0.1:{port}/env"))
        .send()
        .unwrap()
        .json()
        .unwrap()
}

pub(crate) fn wait_dead(pid: u32) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn sidecar_starts_healthy_on_loopback_and_dies_with_its_owner() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let cfg = fake_config(dir.path(), "ok");
    let libs = cfg.lib_dir.clone();
    let mut t = RemoteTranscriber::start(cfg).unwrap();
    let pid = t.sidecar().pid().unwrap();
    assert!(process_alive(pid));
    assert!(t.sidecar().port() > 0);

    // Working directory and library path as #116's layout needs.
    let env = get_env(t.sidecar().port());
    let cwd = PathBuf::from(env["cwd"].as_str().unwrap());
    assert_eq!(cwd.canonicalize().unwrap(), libs.canonicalize().unwrap());
    let lib = env["lib"].as_str().unwrap();
    assert!(lib.starts_with(libs.to_str().unwrap()), "{lib}");
    assert_eq!(env["pid"].as_u64(), Some(pid as u64));

    // A multipart WAV goes in, a sanitised transcript comes out.
    let text = t.transcribe(&tone(16_000)).unwrap();
    assert!(
        text.starts_with("heard ") && text.ends_with(" bytes"),
        "{text}"
    );
    assert!(!text.contains("asr_text"));
    let timed = t.transcribe_timed(&tone(16_000)).unwrap();
    assert_eq!(timed.language.as_deref(), Some("en"));
    assert!(timed.words.is_empty(), "the engine estimates timings");

    drop(t);
    assert!(wait_dead(pid), "no zombie after drop");
}

#[test]
fn long_input_is_sent_in_pieces_of_at_most_30_s() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let mut t = RemoteTranscriber::start(fake_config(dir.path(), "ok")).unwrap();
    let text = t.transcribe(&tone(65 * 16_000)).unwrap();
    // Three requests, each well under the size of 30 s of 16-bit audio.
    let sizes: Vec<usize> = text
        .split("heard ")
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().trim_end_matches(" bytes").parse().unwrap())
        .collect();
    assert_eq!(sizes.len(), 3, "{text}");
    assert!(
        sizes.iter().all(|&n| n < 30 * 16_000 * 2 + 1_000),
        "{sizes:?}"
    );
}

#[test]
fn a_crashed_sidecar_is_restarted_transparently() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let mut s = Sidecar::start(fake_config(dir.path(), "crash_once")).unwrap();
    let first = s.pid().unwrap();
    let wav = crate::archive::audio::wav_bytes(&tone(1_600));
    // Dies mid-request; the retry lands on a fresh process.
    let raw = s.transcribe_wav(&wav).unwrap();
    assert!(raw.starts_with("language English<asr_text>heard"), "{raw}");
    let second = s.pid().unwrap();
    assert_ne!(first, second);
    assert!(wait_dead(first));
    // It keeps serving, and a good answer clears the failure count.
    assert!(s.transcribe_wav(&wav).is_ok());
    assert_eq!(s.pid(), Some(second));
    assert_eq!(s.failures, 0);

    // Dead between requests (killed from outside): restarted on next use.
    kill_all();
    assert!(wait_dead(second));
    assert!(s.transcribe_wav(&wav).is_ok());
    assert!(s.pid().is_some_and(|p| p != second));
}

#[test]
fn a_sidecar_that_always_crashes_fails_the_request_then_backs_off() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = fake_config(dir.path(), "crash_always");
    cfg.backoff_base = Duration::from_secs(5); // beyond MAX_INLINE_BACKOFF
    let mut s = Sidecar::start(cfg).unwrap();
    let wav = crate::archive::audio::wav_bytes(&tone(1_600));
    let err = s.transcribe_wav(&wav).unwrap_err();
    assert!(format!("{err:#}").contains("crashed"), "{err:#}");
    // The next try waits for the backoff instead of hammering: fast failure.
    let t = Instant::now();
    let err = s.transcribe_wav(&wav).unwrap_err();
    assert!(t.elapsed() < Duration::from_secs(2));
    assert!(format!("{err:#}").contains("next try in"), "{err:#}");
    assert!(!s.is_running());
}

#[test]
fn startup_failures_are_reported_not_hung() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let t = Instant::now();
    let err = Sidecar::start(fake_config(dir.path(), "exit_now"))
        .err()
        .unwrap();
    assert!(
        format!("{err:#}").contains("exited while starting"),
        "{err:#}"
    );
    assert!(t.elapsed() < Duration::from_secs(20));

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = fake_config(dir.path(), "never_healthy");
    cfg.start_timeout = Duration::from_secs(1);
    let err = Sidecar::start(cfg).err().unwrap();
    assert!(
        format!("{err:#}").contains("not ready after 1 s"),
        "{err:#}"
    );
    // The unhealthy process was not left running.
    assert_eq!(kill_all(), 0);

    let dir = tempfile::tempdir().unwrap();
    let mut cfg = fake_config(dir.path(), "ok");
    cfg.binary = dir.path().join("no-such-binary");
    let err = Sidecar::start(cfg).err().unwrap();
    assert!(format!("{err:#}").contains("could not start"), "{err:#}");
}

#[test]
fn idle_unload_stops_the_sidecar() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let t = RemoteTranscriber::start(fake_config(dir.path(), "ok")).unwrap();
    let pid = t.sidecar().pid().unwrap();
    let slot = Mutex::new(Some(t));
    let last = Mutex::new(Instant::now().checked_sub(Duration::from_secs(10)));
    let threshold = Duration::from_secs(1);
    // In use (a recording or a long-form session): kept.
    assert!(!crate::pipeline::unload_if_idle(
        &slot, &last, threshold, true
    ));
    assert!(process_alive(pid));
    // Idle: the transcriber goes, and its sidecar with it.
    assert!(crate::pipeline::unload_if_idle(
        &slot, &last, threshold, false
    ));
    assert!(wait_dead(pid));
}

#[test]
fn kill_all_stops_every_live_sidecar() {
    let _serial = serial();
    let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Sidecar::start(fake_config(d1.path(), "ok")).unwrap();
    let b = Sidecar::start(fake_config(d2.path(), "ok")).unwrap();
    let pids = [a.pid().unwrap(), b.pid().unwrap()];
    assert_eq!(kill_all(), 2);
    assert!(pids.iter().all(|&p| wait_dead(p)));
    assert!(!a.is_running() && !b.is_running());
    assert_eq!(kill_all(), 0);
    // The exit guard does the same when Tauri drops it.
    let c = Sidecar::start(fake_config(d1.path(), "ok")).unwrap();
    let pid = c.pid().unwrap();
    drop(ExitGuard);
    assert!(wait_dead(pid));
}

// ----------------------------------------------------------------- live --

/// Real sidecar + Qwen3-ASR 1.7B on a FLEURS clip (CC-BY-4.0, #109).
/// Nothing is downloaded; point the variables at existing files:
///
/// ```sh
/// B=…/scratchpad/bench-109
/// SUSSURRO_TEST_LLAMA_SERVER=$B/bin/llama-b11146/llama-server \
/// SUSSURRO_TEST_QWEN3_ASR_DIR=$B/models \
/// SUSSURRO_TEST_FLEURS_WAV=$B/corpus/chunks/it_it/00.wav \
/// cargo test live_qwen3_asr -- --ignored --nocapture
/// ```
///
/// The libs folder defaults to the binary's own folder (upstream archive
/// layout); `SUSSURRO_TEST_LLAMA_LIBS` overrides it (`npm run sidecar`
/// layout: `src-tauri/binaries/llama-server-libs`).
#[test]
#[ignore]
fn live_qwen3_asr_transcribes_a_fleurs_clip() {
    let binary = PathBuf::from(
        std::env::var("SUSSURRO_TEST_LLAMA_SERVER").expect("set SUSSURRO_TEST_LLAMA_SERVER"),
    );
    let libs = std::env::var("SUSSURRO_TEST_LLAMA_LIBS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| binary.parent().unwrap().to_path_buf());
    let models = PathBuf::from(
        std::env::var("SUSSURRO_TEST_QWEN3_ASR_DIR").expect("set SUSSURRO_TEST_QWEN3_ASR_DIR"),
    );
    let wav = PathBuf::from(
        std::env::var("SUSSURRO_TEST_FLEURS_WAV").expect("set SUSSURRO_TEST_FLEURS_WAV"),
    );

    let paths = crate::stt::sidecar::SidecarPaths {
        binary,
        lib_dir: libs,
    };
    let log = tempfile::tempdir().unwrap();
    let cfg = qwen3_asr_config(&paths, &models, Some(log.path().join("llama-server.log")));
    let started = Instant::now();
    let mut t = RemoteTranscriber::start(cfg).unwrap();
    println!(
        "sidecar healthy after {:.1} s",
        started.elapsed().as_secs_f32()
    );
    let pid = t.sidecar().pid().unwrap();

    let mut stream = crate::audio::decode::FileStream::open(&wav).unwrap();
    let mut samples = Vec::new();
    while let Some(chunk) = stream.next_chunk().unwrap() {
        samples.extend(chunk);
    }
    let started = Instant::now();
    let out = t.transcribe_timed(&samples).unwrap();
    println!(
        "{:.1} s of audio in {:.2} s → [{:?}] {}",
        samples.len() as f32 / 16_000.0,
        started.elapsed().as_secs_f32(),
        out.language,
        out.text
    );
    assert!(!out.text.is_empty());
    assert!(!out.text.contains("asr_text") && !out.text.starts_with("language"));
    // A long dictation (the clip three times, > 30 s) is split and still transcribed.
    let long: Vec<f32> = samples
        .iter()
        .chain(&samples)
        .chain(&samples)
        .copied()
        .collect();
    let text = t.transcribe(&long).unwrap();
    println!("{:.1} s → {text}", long.len() as f32 / 16_000.0);
    assert!(text.len() > out.text.len() * 2);
    drop(t);
    assert!(wait_dead(pid));
}
