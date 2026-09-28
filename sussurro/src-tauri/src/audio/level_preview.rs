//! Live level preview for a picked audio device (#314): while a picker is on
//! screen (New → System audio + mic, Settings → Dictation, Own-voice
//! enrolment) the UI can ask for a moving level of that device *without*
//! recording — no audio is kept, written or sent, only the RMS of roughly
//! the last 100 ms ([`LevelWindow`]).
//!
//! Two independent slots, one per `kind` ("mic", "system"), held in
//! [`LevelPreviews`] (owned by `AppState`). Starting a real recording,
//! dictation or enrolment must call [`LevelPreviews::stop_all`] first so the
//! device is never opened twice.

use super::recorder::{Recorder, TARGET_RATE};
use crate::sources::system::Capture;
use std::collections::VecDeque;
use std::sync::Mutex;

/// ~100 ms at 16 kHz.
pub const WINDOW_SAMPLES: usize = TARGET_RATE as usize / 10;

/// A rolling window of the most recent samples and their RMS — pure, no
/// device access, so it is unit-tested without a real capture.
pub struct LevelWindow {
    buf: VecDeque<f32>,
    cap: usize,
}

impl LevelWindow {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Append new samples, dropping the oldest past `cap`.
    pub fn push(&mut self, samples: &[f32]) {
        for s in samples {
            if self.buf.len() == self.cap {
                self.buf.pop_front();
            }
            self.buf.push_back(*s);
        }
    }

    /// RMS of everything currently held; 0.0 when empty (nothing captured yet).
    pub fn rms(&self) -> f32 {
        if self.buf.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = self.buf.iter().map(|s| s * s).sum();
        (sum_sq / self.buf.len() as f32).sqrt()
    }
}

/// What a preview slot captures from — a live device ([`Recorder`], or a
/// native loopback [`Capture`]) in the app, a scripted stand-in in tests.
pub trait LevelBackend: Send {
    /// Everything captured since the last call.
    fn take(&mut self) -> Vec<f32>;
    /// The device stopped delivering audio (unplugged, denied, closed).
    fn failed(&self) -> bool;
    /// Release the device.
    fn stop(&mut self);
}

impl LevelBackend for Recorder {
    fn take(&mut self) -> Vec<f32> {
        self.take_new_16k()
    }
    fn failed(&self) -> bool {
        self.has_failed()
    }
    fn stop(&mut self) {
        let _ = Recorder::stop(self);
    }
}

impl LevelBackend for Box<dyn Capture> {
    fn take(&mut self) -> Vec<f32> {
        (**self).take()
    }
    fn failed(&self) -> bool {
        (**self).failed()
    }
    fn stop(&mut self) {
        let _ = (**self).stop();
    }
}

/// One open preview stream.
struct Slot {
    backend: Box<dyn LevelBackend>,
    window: LevelWindow,
}

impl Slot {
    fn new(backend: Box<dyn LevelBackend>) -> Self {
        Self {
            backend,
            window: LevelWindow::new(WINDOW_SAMPLES),
        }
    }

    /// `Err` once the device has failed — the caller drops the slot.
    fn level(&mut self) -> Result<f32, ()> {
        if self.backend.failed() {
            return Err(());
        }
        let new = self.backend.take();
        self.window.push(&new);
        Ok(self.window.rms())
    }
}

/// Two independent live previews (mic, system) — never more than one per
/// kind, and never sharing a device with the dictation/session recorder.
#[derive(Default)]
pub struct LevelPreviews {
    mic: Mutex<Option<Slot>>,
    system: Mutex<Option<Slot>>,
}

/// A preview kind: the mic picker, or the system-audio picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Mic,
    System,
}

impl PreviewKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mic" => Some(Self::Mic),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

impl LevelPreviews {
    fn slot(&self, kind: PreviewKind) -> &Mutex<Option<Slot>> {
        match kind {
            PreviewKind::Mic => &self.mic,
            PreviewKind::System => &self.system,
        }
    }

    /// Replace `kind`'s slot, releasing whatever was open first (a restart
    /// on device change, or the same call twice, is safe).
    fn replace(&self, kind: PreviewKind, backend: Box<dyn LevelBackend>) {
        let mut guard = self.slot(kind).lock().unwrap();
        if let Some(mut old) = guard.take() {
            old.backend.stop();
        }
        *guard = Some(Slot::new(backend));
    }

    /// Mic picker: `device` empty = system default, a named device that is
    /// gone falls back to it (same rule as dictation and the mic-test VU
    /// meter).
    pub fn start_mic(&self, device: &str) -> anyhow::Result<()> {
        let mut r = Recorder::default();
        r.start(device)?;
        self.replace(PreviewKind::Mic, Box::new(r));
        Ok(())
    }

    /// System-audio picker on a named input device (a virtual cable, a
    /// monitor source): exact match, never the default-input fallback — a
    /// preview must never silently open the microphone twice.
    pub fn start_system_device(&self, device: &str) -> anyhow::Result<()> {
        let mut r = Recorder::default();
        r.start_exact(device)?;
        self.replace(PreviewKind::System, Box::new(r));
        Ok(())
    }

    /// System-audio picker on the native loopback (#140).
    pub fn start_system_native(&self) -> anyhow::Result<()> {
        let capture = crate::sources::loopback::start()?;
        self.replace(PreviewKind::System, Box::new(capture));
        Ok(())
    }

    /// The current level, or an error once the slot was never started or the
    /// device failed (the slot is then dropped — the next start tries again
    /// cleanly).
    pub fn level(&self, kind: PreviewKind) -> Result<f32, String> {
        let mut guard = self.slot(kind).lock().unwrap();
        let Some(slot) = guard.as_mut() else {
            return Err("no preview is running".to_string());
        };
        match slot.level() {
            Ok(l) => Ok(l),
            Err(()) => {
                *guard = None;
                Err("the device stopped delivering audio".to_string())
            }
        }
    }

    pub fn stop(&self, kind: PreviewKind) {
        if let Some(mut old) = self.slot(kind).lock().unwrap().take() {
            old.backend.stop();
        }
    }

    /// A real recording, dictation or enrolment is about to open a device:
    /// release both previews first so it is never opened twice (#314).
    pub fn stop_all(&self) {
        self.stop(PreviewKind::Mic);
        self.stop(PreviewKind::System);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn window_rms_is_zero_when_empty() {
        let w = LevelWindow::new(4);
        assert_eq!(w.rms(), 0.0);
    }

    #[test]
    fn window_rms_of_a_constant_signal() {
        let mut w = LevelWindow::new(8);
        w.push(&[0.5, 0.5, 0.5, 0.5]);
        assert!((w.rms() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn window_trims_to_capacity() {
        let mut w = LevelWindow::new(2);
        w.push(&[1.0, 1.0, 1.0]); // only the last 2 are kept
        w.push(&[0.0, 0.0]); // silence overwrites them
        assert_eq!(w.rms(), 0.0);
    }

    #[test]
    fn window_silence_after_signal_settles_to_zero() {
        let mut w = LevelWindow::new(4);
        w.push(&[1.0, 1.0, 1.0, 1.0]);
        assert!(w.rms() > 0.9);
        w.push(&[0.0, 0.0, 0.0, 0.0]);
        assert_eq!(w.rms(), 0.0);
    }

    /// A scripted backend for the state-machine tests below — no real device.
    struct FakeBackend {
        chunks: Vec<Vec<f32>>,
        failed: Arc<AtomicBool>,
        stopped: Arc<AtomicBool>,
    }

    impl FakeBackend {
        /// Returns the backend plus the `stopped` flag it will set.
        fn create(chunks: Vec<Vec<f32>>) -> (Box<dyn LevelBackend>, Arc<AtomicBool>) {
            let failed = Arc::new(AtomicBool::new(false));
            let stopped = Arc::new(AtomicBool::new(false));
            (
                Box::new(Self {
                    chunks,
                    failed,
                    stopped: stopped.clone(),
                }),
                stopped,
            )
        }
    }

    impl LevelBackend for FakeBackend {
        fn take(&mut self) -> Vec<f32> {
            if self.chunks.is_empty() {
                Vec::new()
            } else {
                self.chunks.remove(0)
            }
        }
        fn failed(&self) -> bool {
            self.failed.load(Ordering::Relaxed)
        }
        fn stop(&mut self) {
            self.stopped.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn level_before_start_is_an_error() {
        let previews = LevelPreviews::default();
        assert!(previews.level(PreviewKind::Mic).is_err());
    }

    #[test]
    fn level_reflects_the_backend_and_kinds_are_independent() {
        let previews = LevelPreviews::default();
        let (mic_backend, _) = FakeBackend::create(vec![vec![1.0, 1.0, 1.0, 1.0]]);
        previews.replace(PreviewKind::Mic, mic_backend);
        let (sys_backend, _) = FakeBackend::create(vec![vec![0.0, 0.0]]);
        previews.replace(PreviewKind::System, sys_backend);

        assert!(previews.level(PreviewKind::Mic).unwrap() > 0.9);
        assert_eq!(previews.level(PreviewKind::System).unwrap(), 0.0);
    }

    #[test]
    fn a_failed_device_errors_once_and_clears_the_slot() {
        let previews = LevelPreviews::default();
        // A backend that reports itself as already failed.
        let failed = Arc::new(AtomicBool::new(true));
        let stopped = Arc::new(AtomicBool::new(false));
        let failing = FakeBackend {
            chunks: vec![],
            failed,
            stopped,
        };
        previews.replace(PreviewKind::Mic, Box::new(failing));
        assert!(previews.level(PreviewKind::Mic).is_err());
        // The slot was cleared: the next call errors as "not started", not
        // as a repeated device failure.
        assert!(previews
            .level(PreviewKind::Mic)
            .is_err_and(|e| e == "no preview is running"));
    }

    #[test]
    fn starting_again_stops_the_previous_backend() {
        let previews = LevelPreviews::default();
        let (first, stopped) = FakeBackend::create(vec![vec![0.1]]);
        previews.replace(PreviewKind::System, first);
        assert!(!stopped.load(Ordering::Relaxed));
        let (second, _) = FakeBackend::create(vec![vec![0.2]]);
        previews.replace(PreviewKind::System, second);
        assert!(
            stopped.load(Ordering::Relaxed),
            "the old backend must be stopped, not just dropped"
        );
    }

    #[test]
    fn stop_all_releases_both_kinds() {
        let previews = LevelPreviews::default();
        let (mic, mic_stopped) = FakeBackend::create(vec![vec![0.1]]);
        let (sys, sys_stopped) = FakeBackend::create(vec![vec![0.1]]);
        previews.replace(PreviewKind::Mic, mic);
        previews.replace(PreviewKind::System, sys);
        previews.stop_all();
        assert!(mic_stopped.load(Ordering::Relaxed));
        assert!(sys_stopped.load(Ordering::Relaxed));
        assert!(previews.level(PreviewKind::Mic).is_err());
        assert!(previews.level(PreviewKind::System).is_err());
    }

    #[test]
    fn stop_all_is_a_no_op_when_nothing_is_running() {
        let previews = LevelPreviews::default();
        previews.stop_all(); // must not panic
    }

    #[test]
    fn kind_parses_only_the_two_known_names() {
        assert_eq!(PreviewKind::parse("mic"), Some(PreviewKind::Mic));
        assert_eq!(PreviewKind::parse("system"), Some(PreviewKind::System));
        assert_eq!(PreviewKind::parse("other"), None);
    }
}
