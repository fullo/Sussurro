//! The live part of the "Copy diagnostics" text (#101) and the redaction
//! every report goes through: numbers and configuration only, never a
//! transcript, a key or the user's home folder.

use super::ring::{Sample, Stat, Summary};
use super::{SessionBacklog, SidecarStatus, Snapshot};
use std::fmt::Write as _;

/// `812 ms`, `1.4 s`, `2 min 5 s`. Pure.
pub fn fmt_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} min {} s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// `574 MB`, `1.9 GB` (decimal units, like the OS file managers). Pure.
pub fn fmt_bytes(b: u64) -> String {
    if b >= 1_000_000_000 {
        format!("{:.1} GB", b as f64 / 1e9)
    } else if b >= 1_000_000 {
        format!("{:.0} MB", b as f64 / 1e6)
    } else {
        format!("{:.0} KB", b as f64 / 1e3)
    }
}

fn fmt_stat_ms(s: &Stat) -> String {
    format!(
        "median {}, p90 {}",
        fmt_ms(s.median.round() as u64),
        fmt_ms(s.p90.round() as u64)
    )
}

fn phase(label: &str, v: Option<u64>) -> Option<String> {
    v.map(|ms| format!("{label} {}", fmt_ms(ms)))
}

/// One dictation's phases on one line. Pure.
pub fn fmt_dictation(s: &Sample) -> String {
    let parts: Vec<String> = [
        phase("record", s.audio_ms),
        phase("model wait", s.load_ms.filter(|&ms| ms > 0)),
        phase("STT", s.stt_ms),
        phase("cleanup", s.cleanup_ms),
        phase("paste", s.paste_ms),
        phase("Finish→Idle", s.total_ms),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut line = parts.join(" · ");
    if s.failed {
        line.push_str(" (failed)");
    }
    line
}

fn fmt_summary(what: &str, sum: &Summary) -> Option<String> {
    if sum.count == 0 {
        return None;
    }
    let mut parts = Vec::new();
    for (label, st) in [
        ("Finish→Idle", &sum.total),
        ("STT", &sum.stt),
        ("cleanup", &sum.cleanup),
        ("paste", &sum.paste),
    ] {
        if let Some(st) = st {
            parts.push(format!("{label} {}", fmt_stat_ms(st)));
        }
    }
    if let Some(rtf) = &sum.realtime_factor {
        parts.push(format!("real-time factor median {:.2}", rtf.median));
    }
    Some(format!("{what} (last {}): {}", sum.count, parts.join("; ")))
}

fn fmt_backend(b: &Option<super::ComputeBackend>) -> String {
    match b {
        None => "unknown yet".into(),
        Some(b) => {
            let mut s = b.kind.clone();
            if let Some(d) = &b.device {
                let _ = write!(s, " ({d})");
            }
            if let Some(n) = &b.note {
                let _ = write!(s, ", {n}");
            }
            let _ = write!(s, " — from {}", b.source);
            s
        }
    }
}

fn fmt_sidecar(label: &str, s: &SidecarStatus) -> String {
    if !s.available {
        return format!("{label}: not in this build");
    }
    let mut line = format!("{label}: {}", if s.running { "running" } else { "stopped" });
    if let Some(m) = s.memory_bytes {
        let _ = write!(line, " · memory {}", fmt_bytes(m));
    }
    if s.running {
        let _ = write!(line, " · {}", fmt_backend(&s.backend));
    }
    if s.failures > 0 {
        let _ = write!(line, " · {} crash(es)", s.failures);
    }
    line
}

fn fmt_backlog(b: &SessionBacklog) -> String {
    format!(
        "session {}: {:.1} s behind, {} queued, {} segment(s) done",
        b.session_id, b.backlog_s, b.queue_len, b.segments_done
    )
}

/// The live section of the report. Pure.
pub fn live_section(s: &Snapshot) -> String {
    let mut r = String::new();
    let _ = writeln!(r, "Performance (this session only, never saved):");
    match s.dictations.last() {
        Some(d) => {
            let _ = writeln!(r, "  Last dictation: {}", fmt_dictation(d));
        }
        None => {
            let _ = writeln!(r, "  Last dictation: none yet");
        }
    }
    if let Some(line) = fmt_summary("Dictations", &s.dictation_summary) {
        let _ = writeln!(r, "  {line}");
    }
    if let Some(line) = fmt_summary("Long-form segments", &s.segment_summary) {
        let _ = writeln!(r, "  {line}");
    }
    let _ = writeln!(
        r,
        "  STT engine: {} · {} · {} · {}",
        s.stt.engine,
        s.stt.model,
        s.stt.state.replace('_', " "),
        fmt_backend(&s.stt.backend)
    );
    let mut mem = format!(
        "  Memory: app {}",
        s.process.memory_bytes.map_or("n/a".into(), fmt_bytes)
    );
    if let Some(b) = s.stt.model_file_bytes {
        let _ = write!(mem, " · STT model file {}", fmt_bytes(b));
    }
    let _ = writeln!(r, "{mem}");
    let _ = writeln!(
        r,
        "  CPU: {} of {} logical cores",
        s.process
            .cpu_percent
            .map_or("n/a".into(), |p| format!("{p:.0} %")),
        s.process.cpus
    );
    if let Some(sc) = &s.stt.sidecar {
        let _ = writeln!(r, "  {}", fmt_sidecar("Qwen3-ASR sidecar", sc));
    }
    let _ = writeln!(
        r,
        "  {}",
        fmt_sidecar("Bundled LLM sidecar", &s.bundled_llm)
    );
    let _ = writeln!(
        r,
        "  Cleanup last call: {}",
        match &s.cleanup.last_call {
            Some(c) => format!(
                "{} ({}, {} s ago)",
                fmt_ms(c.ms),
                if c.ok { "ok" } else { "failed" },
                c.age_s
            ),
            None => "none yet".into(),
        }
    );
    if s.backlog.is_empty() {
        let _ = writeln!(r, "  Long-form backlog: no session running");
    } else {
        for b in &s.backlog {
            let _ = writeln!(r, "  Long-form backlog: {}", fmt_backlog(b));
        }
    }
    r
}

// ---- redaction -------------------------------------------------------------

fn is_boundary(c: Option<char>) -> bool {
    match c {
        None => true,
        Some(c) => !(c.is_alphanumeric() || c == '_' || c == '-' || c == '.'),
    }
}

/// Replace every occurrence of `needle` in `text` — ASCII case-insensitive,
/// and only where it is not part of a longer name — with `with`. Pure.
fn replace_word(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return text.to_string();
    }
    let lower = text.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&needle_lower) {
        let start = i + pos;
        let end = start + needle.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        let needle_starts_word = needle.chars().next().is_some_and(char::is_alphanumeric);
        let ok_before = !needle_starts_word || is_boundary(before);
        if ok_before && is_boundary(after) {
            out.push_str(&text[i..start]);
            out.push_str(with);
        } else {
            out.push_str(&text[i..end]);
        }
        i = end;
    }
    out.push_str(&text[i..]);
    out
}

/// Remove the user's home folder and account name from a report: the home
/// path (either slash style) becomes `~`, the account name elsewhere
/// (a device named after the user, a path we did not foresee) `<user>`.
/// Names under 3 characters are left alone (too likely to hit ordinary
/// words). Pure.
pub fn redact_home(text: &str, home: Option<&std::path::Path>) -> String {
    let Some(home) = home.and_then(|h| h.to_str()) else {
        return text.to_string();
    };
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    let variants = [
        home.to_string(),
        home.replace('\\', "/"),
        home.replace('/', "\\"),
    ];
    for v in variants.iter() {
        out = replace_word(&out, v, "~");
    }
    let user = home.rsplit(['/', '\\']).next().unwrap_or("");
    if user.chars().count() >= 3 {
        out = replace_word(&out, user, "<user>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn durations_and_sizes_read_naturally() {
        assert_eq!(fmt_ms(0), "0 ms");
        assert_eq!(fmt_ms(812), "812 ms");
        assert_eq!(fmt_ms(1_450), "1.4 s");
        assert_eq!(fmt_ms(125_000), "2 min 5 s");
        assert_eq!(fmt_bytes(574_000_000), "574 MB");
        assert_eq!(fmt_bytes(1_900_000_000), "1.9 GB");
        assert_eq!(fmt_bytes(12_000), "12 KB");
    }

    #[test]
    fn dictation_line_lists_only_the_phases_that_ran() {
        let s = Sample {
            audio_ms: Some(3_200),
            load_ms: Some(0),
            stt_ms: Some(812),
            cleanup_ms: None,
            paste_ms: Some(45),
            total_ms: Some(1_020),
            ..Default::default()
        };
        assert_eq!(
            fmt_dictation(&s),
            "record 3.2 s · STT 812 ms · paste 45 ms · Finish→Idle 1.0 s"
        );
    }

    #[test]
    fn live_section_reports_the_numbers() {
        use crate::diagnostics::*;
        let d = Sample {
            audio_ms: Some(2_000),
            stt_ms: Some(400),
            total_ms: Some(600),
            ..Default::default()
        };
        let snap = Snapshot {
            version: "0.10.0".into(),
            os: "macos aarch64".into(),
            dictations: vec![d],
            dictation_summary: ring::summarize(&[d]),
            segments: vec![],
            segment_summary: Summary::default(),
            stt: SttStatus {
                engine: "whisper".into(),
                model: "ggml-base.bin".into(),
                state: "not_loaded".into(),
                backend: Some(ComputeBackend {
                    kind: "Metal".into(),
                    device: Some("Apple M1 Pro".into()),
                    source: "whisper.cpp log".into(),
                    note: None,
                }),
                model_file_bytes: Some(148_000_000),
                sidecar: None,
            },
            cleanup: CleanupStatus {
                active: true,
                profile: "Local".into(),
                api: "ollama".into(),
                endpoint: "http://localhost:11434".into(),
                external: false,
                last_call: Some(LastCall {
                    ms: 640,
                    ok: true,
                    age_s: 3,
                }),
            },
            bundled_llm: SidecarStatus::default(),
            process: ProcessStatus {
                memory_bytes: Some(812_000_000),
                cpu_percent: Some(12.4),
                cpus: 10,
            },
            backlog: vec![SessionBacklog {
                session_id: 2,
                backlog_s: 4.25,
                processed_s: 30.0,
                queue_len: 1,
                segments_done: 5,
            }],
            recording: false,
        };
        let r = live_section(&snap);
        assert!(
            r.contains("Last dictation: record 2.0 s · STT 400 ms · Finish→Idle 600 ms"),
            "{r}"
        );
        assert!(
            r.contains("Dictations (last 1): Finish→Idle median 600 ms, p90 600 ms"),
            "{r}"
        );
        assert!(r.contains("whisper · ggml-base.bin · not loaded · Metal (Apple M1 Pro) — from whisper.cpp log"), "{r}");
        assert!(
            r.contains("Memory: app 812 MB · STT model file 148 MB"),
            "{r}"
        );
        assert!(r.contains("CPU: 12 % of 10 logical cores"), "{r}");
        assert!(r.contains("Bundled LLM sidecar: not in this build"), "{r}");
        assert!(r.contains("Cleanup last call: 640 ms (ok, 3 s ago)"), "{r}");
        assert!(
            r.contains("session 2: 4.2 s behind, 1 queued, 5 segment(s) done"),
            "{r}"
        );
    }

    #[test]
    fn home_folder_is_redacted_on_unix() {
        let t = "Models dir: /Users/fullo/models\nOutput: /Users/fullo\nOther: /Users/fullo2/x";
        let r = redact_home(t, Some(Path::new("/Users/fullo")));
        assert_eq!(r, "Models dir: ~/models\nOutput: ~\nOther: /Users/fullo2/x");
    }

    #[test]
    fn home_folder_is_redacted_on_windows_either_slash_any_case() {
        let t = r"a C:\Users\Mario\Models b c:/users/mario/x";
        let r = redact_home(t, Some(Path::new(r"C:\Users\Mario")));
        assert_eq!(r, r"a ~\Models b ~/x");
    }

    #[test]
    fn account_name_elsewhere_is_redacted_as_a_word() {
        let t = "Microphone: Fullo's AirPods · Model: fullone-v2";
        let r = redact_home(t, Some(Path::new("/home/fullo")));
        assert_eq!(r, "Microphone: <user>'s AirPods · Model: fullone-v2");
        // Too short to redact safely.
        assert_eq!(
            redact_home("an ad hoc", Some(Path::new("/home/ad"))),
            "an ad hoc"
        );
        assert_eq!(redact_home("x", None), "x");
    }
}
