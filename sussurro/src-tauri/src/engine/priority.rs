//! Hotkey dictation goes before long-form segments (#154).
//!
//! The dictation and the engine share one transcriber (one model in RAM).
//! A dictation is interactive — the user is waiting with the cursor in an
//! app — while a long-form segment is background work, so the engine yields:
//!
//! - the hotkey pipeline calls [`DictationGate::begin`] when it starts
//!   recording and [`DictationGate::end`] once its final transcription is
//!   done (success or not), through [`DictationTurn`];
//! - the engine takes the transcriber through [`acquire_yielding`], which
//!   waits while a dictation is pending and re-checks after acquiring, so
//!   it never starts a new segment ahead of a dictation.
//!
//! **Worst-case wait** for a dictation: a segment already in STT when the
//! hotkey is pressed is not interrupted (whisper.cpp / ONNX Runtime calls
//! can't be pre-empted cleanly), so the dictation's final pass waits for
//! at most the rest of that one segment — i.e. STT time of ≤ 30 s of audio
//! (the segmenter's cap) minus the time the user spent recording. Every
//! later segment waits for the dictation instead.

use std::sync::{Condvar, Mutex};

#[derive(Default)]
struct Inner {
    /// Dictations recording or waiting for their final transcription. A
    /// count, not a flag: a new dictation can start recording while the
    /// previous one is still being transcribed.
    pending: usize,
    /// Engine threads blocked in [`DictationGate::wait_clear`].
    waiting: usize,
}

/// "A hotkey dictation needs the transcriber": lives in `AppState`.
#[derive(Default)]
pub struct DictationGate {
    inner: Mutex<Inner>,
    cv: Condvar,
}

impl DictationGate {
    /// A dictation started recording: no new long-form segment until the
    /// matching [`Self::end`].
    pub fn begin(&self) {
        self.inner.lock().unwrap().pending += 1;
    }

    /// The dictation's final transcription is done (or it was dropped).
    pub fn end(&self) {
        let mut g = self.inner.lock().unwrap();
        g.pending = g.pending.saturating_sub(1);
        if g.pending == 0 {
            self.cv.notify_all();
        }
    }

    pub fn is_pending(&self) -> bool {
        self.inner.lock().unwrap().pending > 0
    }

    /// Block while a dictation is pending. Returns whether it waited.
    pub fn wait_clear(&self) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.pending == 0 {
            return false;
        }
        g.waiting += 1;
        while g.pending > 0 {
            g = self.cv.wait(g).unwrap();
        }
        g.waiting -= 1;
        true
    }

    /// Engine threads currently yielding to a dictation.
    pub fn waiting(&self) -> usize {
        self.inner.lock().unwrap().waiting
    }

    /// Start a dictation turn that ends when the returned value is dropped.
    pub fn turn(&self) -> DictationTurn<'_> {
        self.begin();
        DictationTurn(Some(self))
    }

    /// Adopt a turn already begun with [`Self::begin`] (the hotkey starts
    /// it on one thread, the transcription ends it on another).
    pub fn adopt(&self) -> DictationTurn<'_> {
        DictationTurn(Some(self))
    }
}

/// Ends a dictation turn on drop, so no early return or panic in the
/// dictation pipeline can leave the engine waiting forever.
pub struct DictationTurn<'a>(Option<&'a DictationGate>);

impl DictationTurn<'_> {
    /// End the turn now (the final transcription is done; cleanup and
    /// injection don't need the transcriber).
    pub fn finish(mut self) {
        if let Some(g) = self.0.take() {
            g.end();
        }
    }
}

impl Drop for DictationTurn<'_> {
    fn drop(&mut self) {
        if let Some(g) = self.0.take() {
            g.end();
        }
    }
}

/// Take the transcriber for one long-form segment, yielding to dictation:
/// wait while one is pending, acquire, and if a dictation began in between
/// release and wait again. `acquire` locks (and may load) the model.
pub fn acquire_yielding<G>(
    gate: &DictationGate,
    mut acquire: impl FnMut() -> anyhow::Result<G>,
) -> anyhow::Result<G> {
    loop {
        gate.wait_clear();
        let guard = acquire()?;
        if !gate.is_pending() {
            return Ok(guard);
        }
        drop(guard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Poll `cond` until true; panics after 5 s (a broken gate must fail
    /// the test, not hang it).
    fn eventually(what: &str, cond: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::yield_now();
        }
    }

    #[test]
    fn gate_counts_overlapping_dictations() {
        let g = DictationGate::default();
        assert!(!g.is_pending());
        assert!(!g.wait_clear(), "no dictation: no wait");
        g.begin();
        g.begin(); // a second dictation starts while the first transcribes
        g.end();
        assert!(g.is_pending(), "the second one still needs the model");
        g.end();
        assert!(!g.is_pending());
        g.end(); // unmatched end: never underflows
        assert!(!g.is_pending());
    }

    #[test]
    fn a_turn_ends_on_drop_and_on_finish() {
        let g = DictationGate::default();
        {
            let _t = g.turn();
            assert!(g.is_pending());
        }
        assert!(!g.is_pending(), "dropped (early return / panic)");
        g.begin();
        let t = g.adopt();
        assert!(g.is_pending());
        t.finish();
        assert!(!g.is_pending());
    }

    #[test]
    fn a_waiting_engine_resumes_when_the_dictation_ends() {
        let g = Arc::new(DictationGate::default());
        g.begin();
        let t = {
            let g = g.clone();
            std::thread::spawn(move || g.wait_clear())
        };
        eventually("the engine blocks on the gate", || g.waiting() == 1);
        g.end();
        assert!(t.join().unwrap(), "it waited");
        assert_eq!(g.waiting(), 0);
    }

    /// The core guarantee: a dictation that starts while segment N is being
    /// transcribed is served before segment N + 1.
    #[test]
    fn dictation_is_served_before_the_next_segment() {
        let gate = Arc::new(DictationGate::default());
        let model = Arc::new(Mutex::new(Vec::<String>::new()));
        let (in_seg0_tx, in_seg0_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();

        let engine = {
            let gate = gate.clone();
            let model = model.clone();
            std::thread::spawn(move || {
                for i in 0..3 {
                    let mut m = acquire_yielding(&gate, || Ok(model.lock().unwrap())).unwrap();
                    m.push(format!("segment {i}"));
                    if i == 0 {
                        // A long segment: the hotkey is pressed meanwhile.
                        in_seg0_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                    }
                }
            })
        };

        in_seg0_rx.recv().unwrap();
        let turn = gate.turn(); // hotkey pressed: recording
        release_tx.send(()).unwrap(); // segment 0 finishes
                                      // The engine must now park on the gate instead of taking the model.
        eventually("the engine yields", || gate.waiting() == 1);
        assert_eq!(*model.lock().unwrap(), ["segment 0"]);
        // Recording stops; the final pass takes the model, then ends.
        model.lock().unwrap().push("dictation".into());
        turn.finish();
        engine.join().unwrap();
        assert_eq!(
            *model.lock().unwrap(),
            ["segment 0", "dictation", "segment 1", "segment 2"]
        );
    }

    /// A dictation that begins between the engine's gate check and its lock
    /// still wins: the engine re-checks after acquiring and backs off.
    #[test]
    fn a_dictation_starting_during_acquire_still_goes_first() {
        let gate = Arc::new(DictationGate::default());
        let order = Arc::new(Mutex::new(Vec::<&str>::new()));
        let mut attempts = 0;
        let mut dictation = None;
        let got = acquire_yielding(&gate, || {
            attempts += 1;
            if attempts == 1 {
                gate.begin(); // the hotkey fires as the engine takes the lock
                let (gate, order) = (gate.clone(), order.clone());
                dictation = Some(std::thread::spawn(move || {
                    eventually("the engine backs off", || gate.waiting() == 1);
                    order.lock().unwrap().push("dictation");
                    gate.end();
                }));
            }
            Ok(attempts)
        })
        .unwrap();
        order.lock().unwrap().push("segment");
        dictation.unwrap().join().unwrap();
        assert_eq!(got, 2, "released, waited, re-acquired");
        assert_eq!(*order.lock().unwrap(), ["dictation", "segment"]);
    }
}
