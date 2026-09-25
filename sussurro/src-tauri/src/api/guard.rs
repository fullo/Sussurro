//! Checks every local API request passes before a route runs (#215), and
//! the bounded body reader. Pure — unit tested.
//!
//! - **Host**: only `127.0.0.1:<port>` or `localhost:<port>` (the port the
//!   API listens on). A web page that re-resolves its own name to
//!   127.0.0.1 (DNS rebinding) still sends its own name as `Host`, so it
//!   is refused before any route — it could otherwise read `/history`
//!   "same-origin".
//! - **Origin**: a request carrying one must come from a browser
//!   extension, on every route. Browsers attach an `Origin` to cross-site
//!   POSTs (and to fetches), so a web page can no longer write history or
//!   spend the cleanup LLM with a "simple" POST to `/clean`/`/transcribe`.
//!   Scripts (curl) send none and are unaffected.
//! - **Bodies**: the declared `Content-Length` is checked against the
//!   route's cap before a byte is read, then the body is read through
//!   `take(cap + 1)` into a buffer that never grows past the cap — also
//!   for chunked uploads, which declare no length. The whole body must
//!   arrive within [`BODY_DEADLINE`] (each read also times out after the
//!   server's socket timeout, #223), so a client trickling bytes can't hold
//!   a slot.
//! - **Slots**: `/clean` and `/transcribe` hold one of a few slots while
//!   they run; the rest of the worker pool stays free for the extension's
//!   routes, so a slow transcription can't stall `/app/version` or `/live`.

use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// `POST /clean`: text, 1 MiB is far beyond any dictation.
pub const CLEAN_MAX_BYTES: usize = 1 << 20;
/// `POST /transcribe`: an audio file, 200 MiB (hours of compressed audio).
pub const TRANSCRIBE_MAX_BYTES: usize = 200 << 20;
/// A whole body, however it trickles in (#223). 200 MiB over loopback
/// takes seconds.
pub const BODY_DEADLINE: Duration = Duration::from_secs(120);

/// Is `host` (the `Host` header) this API's own loopback name and port?
/// Case-insensitive; the port may be left out only when it is 80.
pub fn host_allowed(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host.map(str::trim) else {
        return false;
    };
    let (name, given_port) = match host.rsplit_once(':') {
        Some((name, p)) => match p.parse::<u16>() {
            Ok(p) => (name, p),
            Err(_) => return false,
        },
        None => (host, 80),
    };
    given_port == port && (name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost"))
}

/// No `Origin` (a local script) or a browser extension's: allowed on every
/// route. Anything else — a web page, `null` (sandboxed frames, `file:`)
/// — is refused.
pub fn origin_allowed(origin: Option<&str>) -> bool {
    origin.is_none_or(super::auth::is_extension_origin)
}

/// Why a body could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    /// Declared or sent beyond the cap: 413.
    TooLarge,
    /// Nothing sent: 400.
    Empty,
    /// The connection failed mid-body: 400.
    Io,
    /// Not all there by the deadline, or a read timed out: 408.
    TooSlow,
}

/// Read a request body of at most `cap` bytes. A declared length over the
/// cap is refused without reading; otherwise at most `cap + 1` bytes are
/// read, and the buffer never holds (or reserves) more than that. Past
/// `deadline` (checked between reads) the read stops.
pub fn read_capped(
    reader: &mut dyn Read,
    declared: Option<usize>,
    cap: usize,
    deadline: Instant,
) -> Result<Vec<u8>, BodyError> {
    if declared.is_some_and(|n| n > cap) {
        return Err(BodyError::TooLarge);
    }
    if declared == Some(0) {
        return Err(BodyError::Empty);
    }
    let limit = cap.saturating_add(1);
    // A declared length is known to fit: reserve it once. Chunked uploads
    // grow as they arrive, never past the limit.
    let mut buf: Vec<u8> = Vec::with_capacity(declared.unwrap_or(0).min(limit));
    let mut chunk = [0u8; 16 * 1024];
    let mut limited = reader.take(limit as u64);
    loop {
        if Instant::now() > deadline {
            return Err(BodyError::TooSlow);
        }
        let n = match limited.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(BodyError::TooSlow)
            }
            Err(_) => return Err(BodyError::Io),
        };
        let needed = buf.len() + n;
        if needed > buf.capacity() {
            let target = (buf.capacity() * 2).max(needed).max(64 * 1024).min(limit);
            buf.reserve_exact(target - buf.len());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    if buf.len() > cap {
        return Err(BodyError::TooLarge);
    }
    if buf.is_empty() {
        return Err(BodyError::Empty);
    }
    Ok(buf)
}

/// A fixed number of slots for the slow routes. Taking one never blocks:
/// with none free the request is answered 503 at once.
#[derive(Debug)]
pub struct Slots {
    busy: AtomicUsize,
    max: usize,
}

/// A taken slot; freed on drop.
#[derive(Debug)]
pub struct Slot<'a>(&'a Slots);

impl Slots {
    pub fn new(max: usize) -> Self {
        Self {
            busy: AtomicUsize::new(0),
            max,
        }
    }

    pub fn try_take(&self) -> Option<Slot<'_>> {
        // Lazily: a `Slot` built and dropped here would release one.
        self.try_acquire().then(|| Slot(self))
    }

    /// Take a slot without a guard (to hand it to another thread); pair
    /// every `true` with one [`Slots::release`].
    pub fn try_acquire(&self) -> bool {
        self.busy
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |b| {
                (b < self.max).then_some(b + 1)
            })
            .is_ok()
    }

    pub fn release(&self) {
        self.busy.fetch_sub(1, Ordering::AcqRel);
    }

    pub fn busy(&self) -> usize {
        self.busy.load(Ordering::Acquire)
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_apis_loopback_host_is_allowed() {
        for ok in [
            "127.0.0.1:4525",
            "localhost:4525",
            "LocalHost:4525",
            " 127.0.0.1:4525 ",
        ] {
            assert!(host_allowed(Some(ok), 4525), "{ok}");
        }
        for bad in [
            // DNS rebinding: the page's own name, pointed at 127.0.0.1.
            "rebind.attacker:4525",
            "rebind.attacker",
            "127.0.0.1.attacker:4525",
            "localhost.attacker:4525",
            // The right name on another port, or no port.
            "127.0.0.1:4526",
            "127.0.0.1",
            "localhost",
            "127.0.0.1:",
            "127.0.0.1:x",
            // Other loopback spellings the API doesn't listen as.
            "[::1]:4525",
            "127.1:4525",
            "0.0.0.0:4525",
            "",
        ] {
            assert!(!host_allowed(Some(bad), 4525), "{bad}");
        }
        assert!(!host_allowed(None, 4525), "HTTP/1.1 requires Host");
        assert!(host_allowed(Some("localhost"), 80));
    }

    #[test]
    fn only_no_origin_or_an_extension_origin_is_allowed() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("chrome-extension://abcdefghijklmnop")));
        assert!(origin_allowed(Some(
            "moz-extension://2d6b6c1e-3f7a-4b1e-9d0a-1c2b3d4e5f60"
        )));
        for o in [
            "https://evil.example",
            "http://127.0.0.1:4525",
            "http://localhost:4525",
            "null",
            "",
        ] {
            assert!(!origin_allowed(Some(o)), "{o}");
        }
    }

    fn later() -> Instant {
        Instant::now() + BODY_DEADLINE
    }

    /// Counts what is pulled from it; endless unless `len` is set.
    struct Source {
        pulled: usize,
        len: Option<usize>,
    }

    impl Read for Source {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let left = self.len.map_or(usize::MAX, |l| l - self.pulled);
            let n = buf.len().min(left);
            buf[..n].fill(b'a');
            self.pulled += n;
            Ok(n)
        }
    }

    #[test]
    fn a_declared_length_over_the_cap_is_refused_without_reading() {
        let mut src = Source {
            pulled: 0,
            len: None,
        };
        assert_eq!(
            read_capped(&mut src, Some(1_000_001), 1_000_000, later()),
            Err(BodyError::TooLarge)
        );
        assert_eq!(src.pulled, 0);
        assert_eq!(
            read_capped(&mut src, Some(usize::MAX), TRANSCRIBE_MAX_BYTES, later()),
            Err(BodyError::TooLarge)
        );
        assert_eq!(src.pulled, 0);
    }

    #[test]
    fn an_endless_body_stops_one_byte_past_the_cap() {
        // Chunked: no declared length, the sender never stops.
        let mut src = Source {
            pulled: 0,
            len: None,
        };
        assert_eq!(
            read_capped(&mut src, None, 100_000, later()),
            Err(BodyError::TooLarge)
        );
        assert_eq!(src.pulled, 100_001, "never reads past cap + 1");
        // A lying length (fewer declared than sent) is cut the same way.
        let mut src = Source {
            pulled: 0,
            len: None,
        };
        assert_eq!(
            read_capped(&mut src, Some(10), 100_000, later()),
            Err(BodyError::TooLarge)
        );
        assert!(src.pulled <= 100_001);
    }

    #[test]
    fn a_body_up_to_the_cap_is_read_whole_without_over_reserving() {
        let cap = 300_000;
        for (declared, len) in [
            (None, cap),
            (Some(cap), cap),
            (None, 1),
            (Some(5), 5),
            (None, 70_000),
        ] {
            let mut src = Source {
                pulled: 0,
                len: Some(len),
            };
            let body = read_capped(&mut src, declared, cap, later()).unwrap();
            assert_eq!(body.len(), len);
            assert!(
                body.capacity() <= cap + 1,
                "{} reserved for {len}",
                body.capacity()
            );
        }
        let mut empty = Source {
            pulled: 0,
            len: Some(0),
        };
        assert_eq!(
            read_capped(&mut empty, None, cap, later()),
            Err(BodyError::Empty)
        );
        assert_eq!(
            read_capped(&mut empty, Some(0), cap, later()),
            Err(BodyError::Empty)
        );
    }

    /// A client trickling its body stops at the deadline; a read the
    /// socket timed out ends it the same way (#223).
    #[test]
    fn a_body_that_does_not_arrive_in_time_is_refused() {
        struct Trickle(usize);
        impl Read for Trickle {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                std::thread::sleep(Duration::from_millis(5));
                self.0 += 1;
                buf[0] = b'a';
                Ok(1)
            }
        }
        let mut src = Trickle(0);
        let deadline = Instant::now() + Duration::from_millis(100);
        assert_eq!(
            read_capped(&mut src, Some(1000), 1000, deadline),
            Err(BodyError::TooSlow)
        );
        assert!(
            src.0 < 100,
            "stopped at the deadline, after {} reads",
            src.0
        );

        struct TimedOut;
        impl Read for TimedOut {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::WouldBlock.into())
            }
        }
        assert_eq!(
            read_capped(&mut TimedOut, Some(10), 100, later()),
            Err(BodyError::TooSlow)
        );
    }

    #[test]
    fn slots_are_limited_and_freed_on_drop() {
        let slots = Slots::new(2);
        let a = slots.try_take().unwrap();
        let b = slots.try_take().unwrap();
        assert!(slots.try_take().is_none());
        assert_eq!(slots.busy(), 2);
        drop(a);
        let c = slots.try_take().unwrap();
        drop((b, c));
        assert_eq!(slots.busy(), 0);
    }
}
