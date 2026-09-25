use std::io::Read;
use std::io::Result as IoResult;
use std::io::{Error as IoError, ErrorKind};

use super::CloseFlag;

/// A `Reader` that reads exactly the number of bytes from a sub-reader.
///
/// If the limit is reached, it returns EOF.
///
/// Sussurro (#223): upstream read the rest of an unfinished body on drop,
/// into a `vec![0; remaining]` — the size the client *declared*, so an
/// absurd `Content-Length` aborted the process (`memory allocation of …
/// failed`) or panicked (`capacity overflow`), and a silent client held the
/// dropping thread forever. Now a body dropped before its end only sets the
/// connection's [`CloseFlag`]: nothing is read or allocated, and the
/// connection is closed instead of being kept in step. A connection that
/// ends before the declared length is an `UnexpectedEof` error, not a
/// silently short body.
pub struct EqualReader<R>
where
    R: Read,
{
    reader: R,
    size: usize,
    close: CloseFlag,
}

impl<R> EqualReader<R>
where
    R: Read,
{
    pub fn new(reader: R, size: usize, close: CloseFlag) -> EqualReader<R> {
        EqualReader {
            reader,
            size,
            close,
        }
    }
}

impl<R> Read for EqualReader<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if self.size == 0 {
            return Ok(0);
        }

        let buf = if buf.len() < self.size {
            buf
        } else {
            &mut buf[..self.size]
        };
        if buf.is_empty() {
            return Ok(0);
        }

        match self.reader.read(buf) {
            Ok(0) => Err(IoError::new(
                ErrorKind::UnexpectedEof,
                "the connection ended before the declared Content-Length",
            )),
            Ok(len) => {
                self.size -= len;
                Ok(len)
            }
            err @ Err(_) => err,
        }
    }
}

impl<R> Drop for EqualReader<R>
where
    R: Read,
{
    fn drop(&mut self) {
        if self.size > 0 {
            self.close.set();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::CloseFlag;
    use super::EqualReader;
    use std::io::Read;

    #[test]
    fn test_limit() {
        use std::io::Cursor;

        let mut org_reader = Cursor::new("hello world".to_string().into_bytes());
        let close = CloseFlag::default();

        {
            let mut equal_reader = EqualReader::new(org_reader.by_ref(), 5, close.clone());

            let mut string = String::new();
            equal_reader.read_to_string(&mut string).unwrap();
            assert_eq!(string, "hello");
        }

        let mut string = String::new();
        org_reader.read_to_string(&mut string).unwrap();
        assert_eq!(string, " world");
        assert!(!close.is_set(), "read to its end: the connection stays open");
    }

    #[test]
    fn a_body_dropped_unread_closes_the_connection_without_reading() {
        use std::io::Cursor;

        let mut org_reader = Cursor::new("hello world".to_string().into_bytes());
        let close = CloseFlag::default();

        {
            let mut equal_reader = EqualReader::new(org_reader.by_ref(), 5, close.clone());

            let mut vec = [0];
            equal_reader.read_exact(&mut vec).unwrap();
            assert_eq!(vec[0], b'h');
        }

        assert!(close.is_set());
        let mut string = String::new();
        org_reader.read_to_string(&mut string).unwrap();
        assert_eq!(string, "ello world", "nothing was drained");
    }

    #[test]
    fn an_absurd_declared_length_allocates_nothing_on_drop() {
        let close = CloseFlag::default();
        drop(EqualReader::new(std::io::empty(), usize::MAX, close.clone()));
        drop(EqualReader::new(std::io::empty(), isize::MAX as usize, close.clone()));
        assert!(close.is_set());
    }

    #[test]
    fn a_short_body_is_an_error() {
        let close = CloseFlag::default();
        let mut r = EqualReader::new(&b"abc"[..], 10, close);
        let mut s = Vec::new();
        let err = r.read_to_end(&mut s).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        assert_eq!(s, b"abc");
    }
}
