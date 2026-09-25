//! Sussurro (#223): closing a connection whose request body was left unread.
//!
//! Upstream kept such a connection in step by reading the rest of the body
//! when the request dropped. The rest is whatever the client declared (or
//! never ends, for chunked bodies), so it is not read any more: the body
//! reader sets the connection's [`CloseFlag`] instead, and the next read
//! of the connection (the next request's head) fails at once, which ends
//! the connection.

use std::io::{Error as IoError, ErrorKind, Read, Result as IoResult};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Shared by a connection and the body readers of its requests.
#[derive(Clone, Debug, Default)]
pub struct CloseFlag(Arc<AtomicBool>);

impl CloseFlag {
    pub fn set(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// The connection's reader: fails once its [`CloseFlag`] is set.
pub struct Closable<R> {
    inner: R,
    close: CloseFlag,
}

impl<R> Closable<R> {
    pub fn new(inner: R, close: CloseFlag) -> Self {
        Closable { inner, close }
    }
}

impl<R: Read> Read for Closable<R> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if self.close.is_set() {
            return Err(IoError::new(
                ErrorKind::ConnectionAborted,
                "a request body was left unread: connection closed",
            ));
        }
        self.inner.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fail_once_the_flag_is_set() {
        let close = CloseFlag::default();
        let mut r = Closable::new(&b"abcdef"[..], close.clone());
        let mut buf = [0u8; 2];
        assert_eq!(r.read(&mut buf).unwrap(), 2);
        close.set();
        assert_eq!(
            r.read(&mut buf).unwrap_err().kind(),
            ErrorKind::ConnectionAborted
        );
    }
}
