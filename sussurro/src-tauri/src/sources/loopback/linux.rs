//! Linux (#140): the **monitor source of the default sink** — what every
//! PulseAudio sink and every PipeWire sink (through `pipewire-pulse`)
//! offers as "what it plays". Sussurro captures through ALSA (cpal), where
//! monitors are not listed, so the monitor is found with `pactl` and read
//! with `parec`, both from `pulseaudio-utils` (installed with the desktop
//! on the common distros; nothing is linked, so no new build dependency).
//! `parec` asks the server for 16 kHz mono float, so no resampling here.
//!
//! The parsers are pure and tested on every OS; the processes only run on
//! Linux.

use super::Unavailable;
use crate::sources::system::Capture;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// `pactl` gets this long before the server counts as not answering.
const PACTL_TIMEOUT: Duration = Duration::from_secs(3);

/// The sink name `pactl get-default-sink` printed (PulseAudio ≥ 15 and
/// every pipewire-pulse), or `None` for empty output.
pub fn parse_default_sink(out: &str) -> Option<String> {
    let name = out.lines().next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The default sink from `pactl info` (older PulseAudio has no
/// `get-default-sink`); run with `LC_ALL=C`, the labels are translated.
pub fn parse_info_default_sink(out: &str) -> Option<String> {
    out.lines()
        .find_map(|l| l.trim().strip_prefix("Default Sink:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The monitor source of `sink` in `pactl list short sources` (tab
/// separated: index, name, driver, format, state): `<sink>.monitor`.
pub fn monitor_of(sink: &str, sources: &str) -> Option<String> {
    let wanted = format!("{sink}.monitor");
    sources
        .lines()
        .filter_map(|l| l.split('\t').nth(1).map(str::trim))
        .find(|name| *name == wanted)
        .map(str::to_string)
}

/// `name` found in one of `path`'s directories (a `PATH` value).
pub fn find_in_path(name: &str, path: &str) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

fn which(name: &str) -> Option<PathBuf> {
    find_in_path(name, &std::env::var("PATH").unwrap_or_default())
}

/// Run `pactl` (C locale) and return its stdout, or why it failed.
fn pactl(pactl: &PathBuf, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(pactl)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > PACTL_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("pactl did not answer".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err
            .lines()
            .next()
            .unwrap_or("pactl failed")
            .trim()
            .to_string())
    }
}

/// The monitor source of the default sink, or why there is none.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn find_monitor() -> Result<String, Unavailable> {
    let pactl_bin = which("pactl").ok_or(Unavailable::PactlMissing)?;
    if which("parec").is_none() {
        return Err(Unavailable::ParecMissing);
    }
    let sink = match pactl(&pactl_bin, &["get-default-sink"]) {
        Ok(out) => parse_default_sink(&out),
        // PulseAudio < 15: no such command — `pactl info` has it.
        Err(_) => None,
    };
    let sink = match sink {
        Some(s) => s,
        None => {
            let info = pactl(&pactl_bin, &["info"])
                .map_err(|detail| Unavailable::NoSoundServer { detail })?;
            parse_info_default_sink(&info).ok_or(Unavailable::NoOutputDevice)?
        }
    };
    let sources = pactl(&pactl_bin, &["list", "short", "sources"])
        .map_err(|detail| Unavailable::NoSoundServer { detail })?;
    monitor_of(&sink, &sources).ok_or(Unavailable::NoMonitor { sink })
}

/// `parec` recording a monitor source as 16 kHz mono f32 on its stdout.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct ParecCapture {
    child: Child,
    buf: Arc<Mutex<Vec<f32>>>,
    failed: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl ParecCapture {
    pub fn start(monitor: &str) -> anyhow::Result<Self> {
        let parec =
            which("parec").ok_or_else(|| anyhow::anyhow!(Unavailable::ParecMissing.message()))?;
        let mut child = Command::new(parec)
            .args(parec_args(monitor))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("parec has no output"))?;
        let buf: Arc<Mutex<Vec<f32>>> = Arc::default();
        let failed = Arc::new(AtomicBool::new(false));
        let reader = {
            let (buf, failed) = (buf.clone(), failed.clone());
            std::thread::spawn(move || {
                let mut decoder = F32Decoder::default();
                let mut chunk = [0u8; 8192];
                loop {
                    match stdout.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let samples = decoder.push(&chunk[..n]);
                            buf.lock().unwrap().extend_from_slice(&samples);
                        }
                    }
                }
                // parec exited (the server went away, the monitor was
                // removed) or was stopped: nothing more will come.
                failed.store(true, Ordering::Relaxed);
            })
        };
        Ok(Self {
            child,
            buf,
            failed,
            reader: Some(reader),
        })
    }
}

/// `parec`'s arguments: raw 16 kHz mono float from `monitor`, with a short
/// latency so a 250 ms poll always finds audio.
pub fn parec_args(monitor: &str) -> Vec<String> {
    vec![
        format!("--device={monitor}"),
        "--raw".into(),
        "--format=float32le".into(),
        "--rate=16000".into(),
        "--channels=1".into(),
        "--latency-msec=50".into(),
        "--client-name=Sussurro".into(),
        "--stream-name=System audio".into(),
    ]
}

/// Little-endian f32 samples from a byte stream split anywhere.
#[derive(Default)]
pub struct F32Decoder {
    rest: Vec<u8>,
}

impl F32Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<f32> {
        self.rest.extend_from_slice(bytes);
        let whole = self.rest.len() / 4 * 4;
        let out = self.rest[..whole]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        self.rest.drain(..whole);
        out
    }
}

impl Capture for ParecCapture {
    fn take(&mut self) -> Vec<f32> {
        std::mem::take(&mut *self.buf.lock().unwrap())
    }
    fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
    fn stop(&mut self) -> Vec<f32> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        self.take()
    }
    fn gapless(&self) -> bool {
        // A monitor normally delivers zeros while nothing plays; a server
        // that pauses it instead must not end the channel.
        true
    }
}

impl Drop for ParecCapture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `pactl list short sources` on PipeWire (pipewire-pulse 1.0,
    /// Ubuntu 24.04) and on PulseAudio 16.
    const PIPEWIRE_SOURCES: &str = "\
49\talsa_output.pci-0000_00_1f.3.analog-stereo.monitor\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED
50\talsa_input.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED
71\tbluez_output.AC_80_0A_12_34_56.1.monitor\tPipeWire\ts16le 2ch 48000Hz\tRUNNING
";
    const PULSE_SOURCES: &str = "\
0\talsa_output.usb-Generic_USB_Audio-00.analog-stereo.monitor\tmodule-alsa-card.c\ts16le 2ch 44100Hz\tIDLE
1\talsa_input.usb-Generic_USB_Audio-00.mono-fallback\tmodule-alsa-card.c\ts16le 1ch 44100Hz\tSUSPENDED
";

    #[test]
    fn the_default_sink_comes_from_get_default_sink_or_info() {
        assert_eq!(
            parse_default_sink("alsa_output.pci-0000_00_1f.3.analog-stereo\n").as_deref(),
            Some("alsa_output.pci-0000_00_1f.3.analog-stereo")
        );
        assert_eq!(parse_default_sink("\n"), None);
        assert_eq!(parse_default_sink(""), None);
        let info = "\
Server String: /run/user/1000/pulse/native
Library Protocol Version: 35
Server Name: PulseAudio (on PipeWire 1.0.5)
Default Sink: bluez_output.AC_80_0A_12_34_56.1
Default Source: alsa_input.pci-0000_00_1f.3.analog-stereo
Cookie: 1d2e:3f4a
";
        assert_eq!(
            parse_info_default_sink(info).as_deref(),
            Some("bluez_output.AC_80_0A_12_34_56.1")
        );
        assert_eq!(parse_info_default_sink("Server Name: pulseaudio\n"), None);
        assert_eq!(parse_info_default_sink("Default Sink: \n"), None);
    }

    #[test]
    fn the_monitor_of_the_default_sink_is_found() {
        assert_eq!(
            monitor_of(
                "alsa_output.pci-0000_00_1f.3.analog-stereo",
                PIPEWIRE_SOURCES
            )
            .as_deref(),
            Some("alsa_output.pci-0000_00_1f.3.analog-stereo.monitor")
        );
        assert_eq!(
            monitor_of("bluez_output.AC_80_0A_12_34_56.1", PIPEWIRE_SOURCES).as_deref(),
            Some("bluez_output.AC_80_0A_12_34_56.1.monitor")
        );
        assert_eq!(
            monitor_of(
                "alsa_output.usb-Generic_USB_Audio-00.analog-stereo",
                PULSE_SOURCES
            )
            .as_deref(),
            Some("alsa_output.usb-Generic_USB_Audio-00.analog-stereo.monitor")
        );
        // An input is never taken for a monitor, and a sink without one
        // has none.
        assert_eq!(
            monitor_of(
                "alsa_input.pci-0000_00_1f.3.analog-stereo",
                PIPEWIRE_SOURCES
            ),
            None
        );
        assert_eq!(monitor_of("auto_null", PULSE_SOURCES), None);
        assert_eq!(monitor_of("x", ""), None);
    }

    #[test]
    fn parec_records_the_monitor_as_16k_mono_float() {
        let args = parec_args("alsa_output.pci-0000_00_1f.3.analog-stereo.monitor");
        assert!(args
            .contains(&"--device=alsa_output.pci-0000_00_1f.3.analog-stereo.monitor".to_string()));
        for a in [
            "--raw",
            "--format=float32le",
            "--rate=16000",
            "--channels=1",
        ] {
            assert!(args.iter().any(|x| x == a), "{a}");
        }
    }

    #[test]
    fn samples_split_across_reads_are_decoded() {
        let mut d = F32Decoder::default();
        let bytes: Vec<u8> = [0.5f32, -0.25, 1.0]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        assert_eq!(d.push(&bytes[..5]), vec![0.5]);
        assert_eq!(d.push(&bytes[5..7]), Vec::<f32>::new());
        assert_eq!(d.push(&bytes[7..]), vec![-0.25, 1.0]);
    }

    #[test]
    fn tools_are_found_on_the_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pactl"), "").unwrap();
        let path = std::env::join_paths([std::path::Path::new("/nonexistent"), dir.path()])
            .unwrap()
            .into_string()
            .unwrap();
        assert_eq!(find_in_path("pactl", &path), Some(dir.path().join("pactl")));
        assert_eq!(find_in_path("parec", &path), None);
        assert_eq!(find_in_path("pactl", ""), None);
    }
}
