use ascii::AsciiString;

use std::io::Error as IoError;
use std::io::Result as IoResult;
use std::io::{BufReader, BufWriter, ErrorKind, Read};

use std::net::SocketAddr;
use std::str::FromStr;

use crate::common::{HTTPVersion, Header, Method};
use crate::connection::Connection;
use crate::util::RefinedTcpStream;
use crate::util::{Closable, CloseFlag};
use crate::util::{SequentialReader, SequentialReaderBuilder, SequentialWriterBuilder};
use crate::Request;

/// A ClientConnection is an object that will store a socket to a client
/// and return Request objects.
pub struct ClientConnection {
    // address of the client
    remote_addr: IoResult<Option<SocketAddr>>,

    // sequence of Readers to the stream, so that the data is not read in
    //  the wrong order
    source: SequentialReaderBuilder<Closable<BufReader<RefinedTcpStream>>>,

    // sequence of Writers to the stream, to avoid writing response #2 before
    //  response #1
    sink: SequentialWriterBuilder<BufWriter<RefinedTcpStream>>,

    // Reader to read the next header from
    next_header_source: SequentialReader<Closable<BufReader<RefinedTcpStream>>>,

    // Sussurro (#223): set when a request body is left unread; the
    // connection then closes (`util::close_flag`).
    close: CloseFlag,

    // Sussurro (#223): the socket, to lift its timeouts on an upgrade.
    socket: Option<Connection>,

    // set to true if we know that the previous request is the last one
    no_more_requests: bool,

    // true if the connection goes through SSL
    secure: bool,
}

/// Error that can happen when reading a request.
#[derive(Debug)]
enum ReadError {
    WrongRequestLine,
    WrongHeader(HTTPVersion),
    /// the client sent an unrecognized `Expect` header
    ExpectationFailed(HTTPVersion),
    ReadIoError(IoError),
    /// Sussurro (#223): request line + headers over `MAX_HEAD_BYTES`, or more
    /// than `MAX_HEADERS` headers.
    HeadTooLarge,
}

/// Sussurro (#223): the request line and headers together, CRLFs included.
/// Upstream buffered a line until its CRLF and any number of headers.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Sussurro (#223): header lines in one request.
pub const MAX_HEADERS: usize = 100;

/// Sussurro (#223): is the body framing unambiguous (RFC 9112 §6.3)? Every
/// `Content-Length` is digits only and they all agree (upstream took the
/// first one that parsed and read an unparsable one as "no body", leaving
/// the body to be read as the next request); `Transfer-Encoding` ends in
/// `chunked` (upstream decoded any value as chunked) and never comes with
/// a `Content-Length`.
fn framing_is_valid(headers: &[Header]) -> bool {
    let mut length: Option<&str> = None;
    for h in headers.iter().filter(|h| h.field.equiv("Content-Length")) {
        let v = h.value.as_str().trim();
        if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) || v.parse::<usize>().is_err() {
            return false;
        }
        match length {
            Some(l) if l.trim_start_matches('0') != v.trim_start_matches('0') => return false,
            _ => length = Some(v),
        }
    }
    let mut encodings = headers
        .iter()
        .filter(|h| h.field.equiv("Transfer-Encoding"))
        .peekable();
    if encodings.peek().is_none() {
        return true;
    }
    let last = encodings
        .last()
        .and_then(|h| h.value.as_str().rsplit(',').next().map(str::trim));
    length.is_none() && last.map_or(false, |l| l.eq_ignore_ascii_case("chunked"))
}

impl ClientConnection {
    /// Creates a new `ClientConnection` that takes ownership of the `TcpStream`.
    pub fn new(
        write_socket: RefinedTcpStream,
        mut read_socket: RefinedTcpStream,
        socket: Option<Connection>,
    ) -> ClientConnection {
        let remote_addr = read_socket.peer_addr();
        let secure = read_socket.secure();

        let close = CloseFlag::default();
        let mut source = SequentialReaderBuilder::new(Closable::new(
            BufReader::with_capacity(1024, read_socket),
            close.clone(),
        ));
        let first_header = source.next().unwrap();

        ClientConnection {
            source,
            sink: SequentialWriterBuilder::new(BufWriter::with_capacity(1024, write_socket)),
            remote_addr,
            next_header_source: first_header,
            close,
            socket,
            no_more_requests: false,
            secure,
        }
    }

    /// true if the connection is HTTPS
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// Reads the next line from self.next_header_source.
    ///
    /// Reads until `CRLF` is reached. The next read will start
    ///  at the first byte of the new line.
    ///
    /// Sussurro (#223): `budget` is what is left of `MAX_HEAD_BYTES`; `Ok(None)`
    /// once it is spent.
    fn read_next_line(&mut self, budget: &mut usize) -> IoResult<Option<AsciiString>> {
        let mut buf = Vec::new();
        let mut prev_byte_was_cr = false;

        loop {
            if *budget == 0 {
                return Ok(None);
            }
            *budget -= 1;

            let byte = self.next_header_source.by_ref().bytes().next();

            let byte = match byte {
                Some(b) => b?,
                None => return Err(IoError::new(ErrorKind::ConnectionAborted, "Unexpected EOF")),
            };

            if byte == b'\n' && prev_byte_was_cr {
                buf.pop(); // removing the '\r'
                return AsciiString::from_ascii(buf)
                    .map(Some)
                    .map_err(|_| IoError::new(ErrorKind::InvalidInput, "Header is not in ASCII"));
            }

            prev_byte_was_cr = byte == b'\r';

            buf.push(byte);
        }
    }

    /// Reads a request from the stream.
    /// Blocks until the header has been read.
    fn read(&mut self) -> Result<Request, ReadError> {
        let mut budget = MAX_HEAD_BYTES;
        let (method, path, version, headers) = {
            // reading the request line
            let (method, path, version) = {
                let line = self
                    .read_next_line(&mut budget)
                    .map_err(ReadError::ReadIoError)?
                    .ok_or(ReadError::HeadTooLarge)?;

                parse_request_line(
                    line.as_str().trim(), // TODO: remove this conversion
                )?
            };

            // getting all headers
            let headers = {
                let mut headers = Vec::new();
                loop {
                    let line = self
                        .read_next_line(&mut budget)
                        .map_err(ReadError::ReadIoError)?
                        .ok_or(ReadError::HeadTooLarge)?;

                    if line.is_empty() {
                        break;
                    };
                    if headers.len() == MAX_HEADERS {
                        return Err(ReadError::HeadTooLarge);
                    }
                    headers.push(match FromStr::from_str(line.as_str().trim()) {
                        // TODO: remove this conversion
                        Ok(h) => h,
                        _ => return Err(ReadError::WrongHeader(version)),
                    });
                }

                headers
            };

            (method, path, version, headers)
        };

        if !framing_is_valid(&headers) {
            return Err(ReadError::WrongHeader(version));
        }

        // building the writer for the request
        let writer = self.sink.next().unwrap();

        // follow-up for next potential request
        let mut data_source = self.source.next().unwrap();
        std::mem::swap(&mut self.next_header_source, &mut data_source);

        // building the next reader
        let request = crate::request::new_request(
            self.secure,
            method,
            path,
            version.clone(),
            headers,
            *self.remote_addr.as_ref().unwrap(),
            data_source,
            writer,
            self.close.clone(),
        )
        .map_err(|e| {
            use crate::request;
            match e {
                request::RequestCreationError::CreationIoError(e) => ReadError::ReadIoError(e),
                request::RequestCreationError::ExpectationFailed => {
                    ReadError::ExpectationFailed(version)
                }
            }
        })?;

        // return the request
        Ok(request)
    }
}

impl Iterator for ClientConnection {
    type Item = Request;

    /// Blocks until the next Request is available.
    /// Returns None when no new Requests will come from the client.
    fn next(&mut self) -> Option<Request> {
        use crate::{Response, StatusCode};

        // the client sent a "connection: close" header in this previous request
        //  or is using HTTP 1.0, meaning that no new request will come
        if self.no_more_requests {
            return None;
        }

        loop {
            let rq = match self.read() {
                Err(ReadError::WrongRequestLine) => {
                    let writer = self.sink.next().unwrap();
                    let response = Response::new_empty(StatusCode(400));
                    response
                        .raw_print(writer, HTTPVersion(1, 1), &[], false, None)
                        .ok();
                    return None; // we don't know where the next request would start,
                                 // se we have to close
                }

                Err(ReadError::WrongHeader(ver)) => {
                    let writer = self.sink.next().unwrap();
                    let response = Response::new_empty(StatusCode(400));
                    response.raw_print(writer, ver, &[], false, None).ok();
                    return None; // we don't know where the next request would start,
                                 // se we have to close
                }

                // Sussurro (#223): upstream answered 408 to a timed-out head
                // read (unreachable there: it set no timeouts). With the
                // `Limits` timeout this is usually an idle keep-alive
                // connection, possibly while the previous request is still
                // being answered, so a 408 would trail that answer: close
                // silently, as for `WouldBlock` (how macOS/Linux report it).

                Err(ReadError::ExpectationFailed(ver)) => {
                    let writer = self.sink.next().unwrap();
                    let response = Response::new_empty(StatusCode(417));
                    response.raw_print(writer, ver, &[], true, None).ok();
                    return None; // TODO: should be recoverable, but needs handling in case of body
                }

                Err(ReadError::HeadTooLarge) => {
                    let writer = self.sink.next().unwrap();
                    let response = Response::new_empty(StatusCode(431));
                    response
                        .raw_print(writer, HTTPVersion(1, 1), &[], false, None)
                        .ok();
                    return None; // the rest of the head is not read: closing
                }

                Err(ReadError::ReadIoError(_)) => return None,

                Ok(rq) => rq,
            };

            // checking HTTP version
            if *rq.http_version() > (1, 1) {
                let writer = self.sink.next().unwrap();
                let response = Response::from_string(
                    "This server only supports HTTP versions 1.0 and 1.1".to_owned(),
                )
                .with_status_code(StatusCode(505));
                response
                    .raw_print(writer, HTTPVersion(1, 1), &[], false, None)
                    .ok();
                continue;
            }

            // updating the status of the connection
            let connection_header = rq
                .headers()
                .iter()
                .find(|h| h.field.equiv("Connection"))
                .map(|h| h.value.as_str());

            let lowercase = connection_header.map(|h| h.to_ascii_lowercase());

            match lowercase {
                Some(ref val) if val.contains("close") => self.no_more_requests = true,
                Some(ref val) if val.contains("upgrade") => {
                    self.no_more_requests = true;
                    // Sussurro (#223): an upgraded stream (the app's WebSocket)
                    // waits for messages as long as it likes; the timeouts
                    // the server set at accept are for HTTP only.
                    if let Some(socket) = &self.socket {
                        socket.set_timeouts(None).ok();
                    }
                }
                Some(ref val)
                    if !val.contains("keep-alive") && *rq.http_version() == HTTPVersion(1, 0) =>
                {
                    self.no_more_requests = true
                }
                None if *rq.http_version() == HTTPVersion(1, 0) => self.no_more_requests = true,
                _ => (),
            };

            // returning the request
            return Some(rq);
        }
    }
}

/// Parses a "HTTP/1.1" string.
fn parse_http_version(version: &str) -> Result<HTTPVersion, ReadError> {
    let (major, minor) = match version {
        "HTTP/0.9" => (0, 9),
        "HTTP/1.0" => (1, 0),
        "HTTP/1.1" => (1, 1),
        "HTTP/2.0" => (2, 0),
        "HTTP/3.0" => (3, 0),
        _ => return Err(ReadError::WrongRequestLine),
    };

    Ok(HTTPVersion(major, minor))
}

/// Parses the request line of the request.
/// eg. GET / HTTP/1.1
fn parse_request_line(line: &str) -> Result<(Method, String, HTTPVersion), ReadError> {
    let mut parts = line.split(' ');

    let method = parts.next().and_then(|w| w.parse().ok());
    let path = parts.next().map(ToOwned::to_owned);
    let version = parts.next().and_then(|w| parse_http_version(w).ok());

    method
        .and_then(|method| Some((method, path?, version?)))
        .ok_or(ReadError::WrongRequestLine)
}

#[cfg(test)]
mod test {
    #[test]
    fn test_parse_request_line() {
        let (method, path, ver) = super::parse_request_line("GET /hello HTTP/1.1").unwrap();

        assert!(method == crate::Method::Get);
        assert!(path == "/hello");
        assert!(ver == crate::common::HTTPVersion(1, 1));

        assert!(super::parse_request_line("GET /hello").is_err());
        assert!(super::parse_request_line("qsd qsd qsd").is_err());
    }

    /// Sussurro (#223).
    #[test]
    fn framing_must_be_unambiguous() {
        let h = |list: &[&str]| -> Vec<crate::Header> {
            list.iter().map(|l| l.parse().unwrap()).collect()
        };
        for ok in [
            &[][..],
            &["Content-Length: 0"],
            &["Content-Length: 18446744073709551615"],
            &["Content-Length: 5", "Content-Length: 005"],
            &["Transfer-Encoding: chunked"],
            &["Transfer-Encoding: gzip, Chunked"],
        ] {
            assert!(super::framing_is_valid(&h(ok)), "{:?}", ok);
        }
        for bad in [
            &["Content-Length: 18446744073709551616"][..],
            &["Content-Length: -1"],
            &["Content-Length: +5"],
            &["Content-Length: 5 5"],
            &["Content-Length: 0x10"],
            &["Content-Length: 5", "Content-Length: 6"],
            &["Transfer-Encoding: gzip"],
            &["Transfer-Encoding: chunked, gzip"],
            &["Transfer-Encoding: chunked", "Content-Length: 5"],
        ] {
            assert!(!super::framing_is_valid(&h(bad)), "{:?}", bad);
        }
    }
}
