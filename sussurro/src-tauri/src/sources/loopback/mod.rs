//! "This computer's sound (built-in)": the second channel of
//! *System audio + mic* captured natively, with no virtual device (0.10,
//! #140, plan §4.1 step 2):
//!
//! - **Windows**: WASAPI loopback on the default output device, through
//!   cpal 0.16 (an input stream on an output device opens in loopback mode —
//!   [`Recorder::start_output_loopback`]). Delivers nothing while nothing
//!   plays, hence [`Capture::gapless`].
//! - **macOS 14.2+**: a Core Audio process tap on every process but ours,
//!   read through a private aggregate device ([`macos`]). The OS asks for
//!   the "System Audio Recording" permission on first use
//!   (`NSAudioCaptureUsageDescription`). The app's minimum stays 11.0: the
//!   tap API is looked up at run time and the choice is hidden below 14.2.
//! - **Linux**: the monitor source of the default sink (PulseAudio or
//!   PipeWire through `pipewire-pulse`), found with `pactl` and read with
//!   `parec` (both from `pulseaudio-utils`), already converted by the sound
//!   server to 16 kHz mono ([`linux`]).
//!
//! Whenever it is unavailable, [`probe`] says why and the UI falls back to
//! the device picker of step 1. No echo cancellation: on speakers the mic
//! channel hears the others too, and the UI suggests headphones.

pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod version;

use super::system::Capture;
use crate::audio::recorder::Recorder;
use serde::Serialize;

/// The picker's label for the native choice.
pub const NATIVE_LABEL: &str = "This computer's sound (built-in)";

/// Why the native capture can't be used here — the UI shows the message
/// and offers the device picker instead.
#[derive(Debug, Clone, PartialEq)]
pub enum Unavailable {
    /// macOS older than 14.2 (process taps).
    MacOsTooOld { version: String },
    /// macOS 14.2+ but the tap API could not be found.
    TapApiMissing,
    /// No output device to capture.
    NoOutputDevice,
    /// Linux: `pactl` is not installed.
    PactlMissing,
    /// Linux: `parec` is not installed.
    ParecMissing,
    /// Linux: no PulseAudio / PipeWire-Pulse server answered.
    NoSoundServer { detail: String },
    /// Linux: the default output has no monitor source.
    NoMonitor { sink: String },
    /// Not built for this OS.
    Unsupported,
}

impl Unavailable {
    pub fn message(&self) -> String {
        match self {
            Unavailable::MacOsTooOld { version } => format!(
                "recording the computer's sound directly needs macOS 14.2 or later (this Mac runs {version}) — use a loopback device such as BlackHole"
            ),
            Unavailable::TapApiMissing => {
                "the Core Audio tap API is missing on this Mac — use a loopback device such as BlackHole".into()
            }
            Unavailable::NoOutputDevice => {
                "there is no output device to record — connect speakers or headphones, or choose an input device".into()
            }
            Unavailable::PactlMissing => {
                "pactl was not found — install pulseaudio-utils (it works with PulseAudio and PipeWire), or choose a monitor device".into()
            }
            Unavailable::ParecMissing => {
                "parec was not found — install pulseaudio-utils (it works with PulseAudio and PipeWire), or choose a monitor device".into()
            }
            Unavailable::NoSoundServer { detail } => format!(
                "no PulseAudio or PipeWire sound server answered ({detail}) — choose an input device"
            ),
            Unavailable::NoMonitor { sink } => format!(
                "the default output '{sink}' has no monitor source — choose an input device"
            ),
            Unavailable::Unsupported => {
                "recording the computer's sound directly is not supported on this system — choose an input device".into()
            }
        }
    }
}

/// `list_system_audio_devices.native`: whether the native choice is offered.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NativeLoopback {
    pub available: bool,
    /// `wasapi` | `coreaudio-tap` | `pulse-monitor` | `none`.
    pub backend: &'static str,
    /// What it records, when known: the output device, or the monitor
    /// source.
    pub detail: Option<String>,
    /// Why it is unavailable (the device picker is the fallback).
    pub reason: Option<String>,
    /// The OS asks for a permission on first use (macOS).
    pub needs_permission: bool,
}

impl NativeLoopback {
    fn ok(backend: &'static str, detail: Option<String>, needs_permission: bool) -> Self {
        Self {
            available: true,
            backend,
            detail,
            reason: None,
            needs_permission,
        }
    }

    fn unavailable(backend: &'static str, why: &Unavailable) -> Self {
        Self {
            available: false,
            backend,
            detail: None,
            reason: Some(why.message()),
            needs_permission: false,
        }
    }
}

/// Whether native capture works here, and on what. Cheap enough for every
/// listing (Linux runs `pactl` twice).
pub fn probe() -> NativeLoopback {
    #[cfg(target_os = "windows")]
    {
        match crate::audio::recorder::default_output_device_name() {
            Some(name) => NativeLoopback::ok("wasapi", Some(name), false),
            None => NativeLoopback::unavailable("wasapi", &Unavailable::NoOutputDevice),
        }
    }
    #[cfg(target_os = "macos")]
    {
        match macos::check() {
            Ok(detail) => NativeLoopback::ok("coreaudio-tap", detail, true),
            Err(why) => NativeLoopback::unavailable("coreaudio-tap", &why),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match linux::find_monitor() {
            Ok(monitor) => NativeLoopback::ok("pulse-monitor", Some(monitor), false),
            Err(why) => NativeLoopback::unavailable("pulse-monitor", &why),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        NativeLoopback::unavailable("none", &Unavailable::Unsupported)
    }
}

/// Open the native capture of the computer's sound, already 16 kHz mono.
pub fn start() -> anyhow::Result<Box<dyn Capture>> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(WasapiLoopback::start()?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::TapCapture::start()?))
    }
    #[cfg(target_os = "linux")]
    {
        let monitor = linux::find_monitor().map_err(|why| anyhow::anyhow!(why.message()))?;
        Ok(Box::new(linux::ParecCapture::start(&monitor)?))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        anyhow::bail!(Unavailable::Unsupported.message())
    }
}

/// WASAPI loopback through a [`Recorder`] on the default output device.
/// Compiled on every OS (only cpal's portable API) so the Windows path is
/// type-checked by every build, but only Windows uses it: other cpal hosts
/// refuse an input stream on an output device.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
struct WasapiLoopback(Recorder);

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl WasapiLoopback {
    fn start() -> anyhow::Result<Self> {
        if crate::audio::recorder::default_output_device_name().is_none() {
            anyhow::bail!(Unavailable::NoOutputDevice.message());
        }
        let mut r = Recorder::default();
        r.start_output_loopback()?;
        Ok(Self(r))
    }
}

impl Capture for WasapiLoopback {
    fn take(&mut self) -> Vec<f32> {
        self.0.take_new_16k()
    }
    fn failed(&self) -> bool {
        self.0.has_failed()
    }
    fn stop(&mut self) -> Vec<f32> {
        if !self.0.is_recording() {
            return Vec::new();
        }
        self.0.stop().unwrap_or_else(|e| {
            eprintln!("system audio: loopback stopped with an error ({e:#})");
            Vec::new()
        })
    }
    fn gapless(&self) -> bool {
        // WASAPI sends no packets while nothing is rendered.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_points_to_the_device_picker_or_a_fix() {
        let all = [
            Unavailable::MacOsTooOld {
                version: "13.6.1".into(),
            },
            Unavailable::TapApiMissing,
            Unavailable::NoOutputDevice,
            Unavailable::PactlMissing,
            Unavailable::ParecMissing,
            Unavailable::NoSoundServer {
                detail: "Connection refused".into(),
            },
            Unavailable::NoMonitor {
                sink: "alsa_output.usb".into(),
            },
            Unavailable::Unsupported,
        ];
        for why in &all {
            let m = why.message();
            assert!(
                m.contains("choose") || m.contains("BlackHole") || m.contains("install"),
                "{m}"
            );
        }
        assert!(all[0].message().contains("14.2") && all[0].message().contains("13.6.1"));
        assert!(all[5].message().contains("Connection refused"));
        assert!(all[6].message().contains("alsa_output.usb"));
    }

    #[test]
    fn an_unavailable_probe_carries_the_reason() {
        let p = NativeLoopback::unavailable("pulse-monitor", &Unavailable::PactlMissing);
        assert!(!p.available && p.detail.is_none());
        assert!(p.reason.unwrap().contains("pulseaudio-utils"));
        let ok = NativeLoopback::ok("coreaudio-tap", None, true);
        assert!(ok.available && ok.reason.is_none() && ok.needs_permission);
    }

    /// The real native capture: play something (a video, `afplay`,
    /// `paplay`) while it runs. Run manually on each OS:
    /// `cargo test native_capture_hears_the_computer -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn native_capture_hears_the_computer() {
        let probe = probe();
        println!("probe: {probe:?}");
        assert!(probe.available, "{:?}", probe.reason);
        let mut c = start().unwrap();
        let mut got = Vec::new();
        for _ in 0..16 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            got.extend(c.take());
            assert!(!c.failed(), "the capture failed");
        }
        got.extend(c.stop());
        let peak = got.iter().fold(0f32, |m, x| m.max(x.abs()));
        println!(
            "captured {} samples at 16 kHz, peak {peak:.4}, gapless {}",
            got.len(),
            c.gapless()
        );
        assert!(
            peak > 0.001,
            "only silence — is something playing? permission granted?"
        );
    }
}
