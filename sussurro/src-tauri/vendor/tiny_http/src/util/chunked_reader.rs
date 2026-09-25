//! Sussurro (#223): `Transfer-Encoding: chunked` request bodies.
//!
//! Replaces `chunked_transfer::Decoder` for requests, which buffered a
//! chunk-size line until its `\r` with no limit (an endless line grew
//! memory without producing a byte of body) and, like the other body
//! readers, left an unfinished body to be read by whoever came next. Here
//! every framing line (chunk size with its extensions, trailers) is at most
//! [`MAX_LINE`] bytes, there are at most [`MAX_TRAILERS`] trailer lines, and
//! a body dropped before its last chunk sets the connection's
//! [`CloseFlag`].

use std::io::{Error as IoError, ErrorKind, Read, Result as IoResult};

use super::CloseFlag;

/// A chunk-size line (with extensions) or a trailer line, CRLF included.
pub const MAX_LINE: usize = 4096;
/// Trailer lines after the last chunk.
pub const MAX_TRAILERS: usize = 64;

#[derive(Debug, PartialEq, Eq)]
enum State {
    /// Next: a chunk-size line.
    Size,
    /// Inside a chunk: this many data bytes left, then CRLF.
    Data(usize),
    /// The last chunk and its trailers were read.
    Done,
}

pub struct ChunkedReader<R: Read> {
    source: R,
    state: State,
    close: CloseFlag,
}

fn invalid(msg: &'static str) -> IoError {
    IoError::new(ErrorKind::InvalidData, msg)
}

impl<R: Read> ChunkedReader<R> {
    pub fn new(source: R, close: CloseFlag) -> Self {
        ChunkedReader {
            source,
            state: State::Size,
            close,
        }
    }

    fn byte(&mut self) -> IoResult<u8> {
        let mut b = [0u8; 1];
        loop {
            match self.source.read(&mut b) {
                Ok(0) => {
                    return Err(IoError::new(
                        ErrorKind::UnexpectedEof,
                        "the connection ended inside a chunked body",
                    ))
                }
                Ok(_) => return Ok(b[0]),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// One CRLF-terminated line, without the CRLF; at most [`MAX_LINE`].
    fn line(&mut self) -> IoResult<Vec<u8>> {
        let mut line = Vec::new();
        loop {
            let b = self.byte()?;
            if b == b'\n' {
                if line.pop() != Some(b'\r') {
                    return Err(invalid("chunked body: a line must end with CRLF"));
                }
                return Ok(line);
            }
            if line.len() + 1 >= MAX_LINE {
                return Err(invalid("chunked body: framing line too long"));
            }
            line.push(b);
        }
    }

    fn chunk_size(&mut self) -> IoResult<usize> {
        let line = self.line()?;
        let size = line.split(|&b| b == b';').next().unwrap_or_default();
        let size = std::str::from_utf8(size)
            .map_err(|_| invalid("chunked body: bad chunk size"))?
            .trim_matches(|c| c == ' ' || c == '\t');
        if size.is_empty() || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("chunked body: bad chunk size"));
        }
        usize::from_str_radix(size, 16).map_err(|_| invalid("chunked body: chunk size too large"))
    }
}

impl<R: Read> Read for ChunkedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        loop {
            match self.state {
                State::Done => return Ok(0),
                State::Size => {
                    let size = self.chunk_size()?;
                    if size == 0 {
                        let mut trailers = 0;
                        while !self.line()?.is_empty() {
                            trailers += 1;
                            if trailers > MAX_TRAILERS {
                                return Err(invalid("chunked body: too many trailers"));
                            }
                        }
                        self.state = State::Done;
                        return Ok(0);
                    }
                    self.state = State::Data(size);
                }
                State::Data(left) => {
                    if buf.is_empty() {
                        return Ok(0);
                    }
                    let want = buf.len().min(left);
                    let n = match self.source.read(&mut buf[..want]) {
                        Ok(0) => {
                            return Err(IoError::new(
                                ErrorKind::UnexpectedEof,
                                "the connection ended inside a chunked body",
                            ))
                        }
                        Ok(n) => n,
                        Err(e) => return Err(e),
                    };
                    if n == left {
                        if self.byte()? != b'\r' || self.byte()? != b'\n' {
                            return Err(invalid("chunked body: chunk data must end with CRLF"));
                        }
                        self.state = State::Size;
                    } else {
                        self.state = State::Data(left - n);
                    }
                    return Ok(n);
                }
            }
        }
    }
}

impl<R: Read> Drop for ChunkedReader<R> {
    fn drop(&mut self) {
        if self.state != State::Done {
            self.close.set();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(input: &[u8]) -> (IoResult<Vec<u8>>, bool) {
        let close = CloseFlag::default();
        let mut out = Vec::new();
        let res = {
            let mut r = ChunkedReader::new(input, close.clone());
            r.read_to_end(&mut out).map(|_| out)
        };
        (res, close.is_set())
    }

    #[test]
    fn decodes_chunks_extensions_and_trailers() {
        let (body, closed) = decode(b"5\r\nhello\r\n6;name=v\r\n world\r\n0\r\nX-T: 1\r\n\r\n");
        assert_eq!(body.unwrap(), b"hello world");
        assert!(!closed);
        let (body, closed) = decode(b"A \r\n0123456789\r\n0\r\n\r\n");
        assert_eq!(body.unwrap(), b"0123456789");
        assert!(!closed);
    }

    #[test]
    fn framing_lines_are_bounded() {
        // An endless chunk-size line: refused after MAX_LINE bytes, not buffered.
        let endless = vec![b'1'; 10 * MAX_LINE];
        let (body, closed) = decode(&endless);
        assert_eq!(body.unwrap_err().kind(), ErrorKind::InvalidData);
        assert!(closed);
        // The same through a long extension.
        let mut ext = b"5;".to_vec();
        ext.extend(vec![b'x'; 2 * MAX_LINE]);
        assert_eq!(decode(&ext).0.unwrap_err().kind(), ErrorKind::InvalidData);
        // Too many trailers.
        let mut t = b"0\r\n".to_vec();
        for _ in 0..=MAX_TRAILERS {
            t.extend_from_slice(b"X: y\r\n");
        }
        t.extend_from_slice(b"\r\n");
        assert_eq!(decode(&t).0.unwrap_err().kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn bad_sizes_and_truncation_are_errors() {
        for bad in [
            &b"zz\r\n"[..],
            b"\r\n",
            b"-1\r\n",
            b"+5\r\nhello\r\n0\r\n\r\n",
            b"fffffffffffffffffff\r\n",
            b"5\nhello\r\n0\r\n\r\n",
            b"5\r\nhelloXX0\r\n\r\n",
        ] {
            let (body, closed) = decode(bad);
            assert!(body.is_err(), "{:?}", String::from_utf8_lossy(bad));
            assert!(closed);
        }
        let (body, closed) = decode(b"5\r\nhel");
        assert_eq!(body.unwrap_err().kind(), ErrorKind::UnexpectedEof);
        assert!(closed);
    }

    #[test]
    fn a_huge_declared_chunk_allocates_nothing() {
        // 2^60 bytes announced, three sent: only what arrives is returned.
        let (body, closed) = decode(b"1000000000000000\r\nabc");
        assert_eq!(body.unwrap_err().kind(), ErrorKind::UnexpectedEof);
        assert!(closed);
    }
}
