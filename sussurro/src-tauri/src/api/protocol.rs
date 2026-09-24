//! The `/live` WebSocket protocol between the browser extension and the app
//! (#126, plan §6). Pure — unit tested.
//!
//! **Client → app**
//! - Text frames, JSON with a `type`:
//!   - `start {title, url, platform, rate, channels}` — begin a meeting:
//!     `rate` is the browser's sample rate (8–192 kHz), `channels` how many
//!     logical channels the client will send (1 or 2: `mic`, `remote`).
//!   - `speaker_active {name, t}` — the meeting page shows `name` speaking;
//!     `t` is milliseconds since `start` on the client's audio clock.
//!   - `participants {names}` — the current participant list.
//!   - `stop` — end the meeting: the app transcribes what is left, writes
//!     the item and closes the socket.
//!   - `ping` — no audio to send right now; the app answers with a
//!     `status`. (Replies go out after each client message, so a client
//!     that sends nothing hears nothing — see `live.rs`.)
//! - Binary frames: audio, `[u8 channel][u32 seq][i16 pcm…]`, little
//!   endian. `channel` is 0 = `mic` (the user's own microphone track) or
//!   1 = `remote` (everyone else); the PCM is mono at the `start` rate.
//!   `seq` counts the frames of one channel from 0: a gap means frames were
//!   lost and is filled with silence of the same length (so the channels
//!   stay aligned); a repeated or older `seq` is dropped. A client that
//!   pauses a channel (track removed) should skip `seq` numbers for the
//!   frames it did not send, or send silence.
//!
//! **App → client** (JSON text frames with a `type`):
//! - `segment {kind: "new" | "updated", segment}` — a transcribed segment.
//! - `speaker {id, label}` — a speaker became known (sent from #131 on).
//! - `status {state, …}` — `ready` (on connect, with `protocol` and `app`),
//!   `started` (item created), `recording` / `finishing` (with the
//!   backlog), `done` (item written), `warning` (a bad message or lost
//!   audio; the session goes on), `error` (the session failed or could not
//!   start).

use crate::archive::{Channel, Segment};
use serde::{Deserialize, Serialize};

/// Bumped on any incompatible change; the extension refuses to run against
/// another protocol (`GET /app/version`).
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest WebSocket message accepted (1 s of 48 kHz audio is 96 KB).
pub const MAX_MESSAGE_BYTES: usize = 512 * 1024;
/// Bytes before the PCM in an audio frame.
pub const FRAME_HEADER: usize = 5;
pub const MIN_RATE: u32 = 8_000;
pub const MAX_RATE: u32 = 192_000;
/// Longest title kept (characters).
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest participant or speaker name kept (characters).
pub const MAX_NAME_CHARS: usize = 120;
/// Most participants kept from one `participants` message.
pub const MAX_PARTICIPANTS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(pub String);

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProtocolError {}

fn bad(msg: impl Into<String>) -> ProtocolError {
    ProtocolError(msg.into())
}

/// `start`, as sent.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StartInfo {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub platform: String,
    pub rate: u32,
    pub channels: u8,
}

/// A validated `start`.
#[derive(Debug, Clone, PartialEq)]
pub struct Start {
    /// Trimmed, one line, at most [`MAX_TITLE_CHARS`]; may be empty (the
    /// engine then titles the item from its first words).
    pub title: String,
    /// The meeting page's host, lowercased: `meet.google.com`.
    pub host: String,
    /// `meet` | `teams` | `zoom` | `other`.
    pub platform: String,
    pub rate: u32,
    pub channels: u8,
}

impl Start {
    /// `browser:<host>` for the frontmatter.
    pub fn source_label(&self) -> String {
        format!("browser:{}", self.host)
    }
}

/// Client → app control messages.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Start(StartInfo),
    SpeakerActive { name: String, t: f64 },
    Participants { names: Vec<String> },
    Stop,
    Ping,
}

/// Parse a text frame.
pub fn parse_control(text: &str) -> Result<ClientMessage, ProtocolError> {
    serde_json::from_str(text).map_err(|e| {
        // serde's message names the field or the unknown `type`, never
        // echoes the payload.
        bad(format!("bad control message: {e}"))
    })
}

fn one_line(s: &str, max: usize) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// A display name from the page: one line, bounded, `None` when empty.
pub fn clean_name(name: &str) -> Option<String> {
    let n = one_line(name, MAX_NAME_CHARS);
    (!n.is_empty()).then_some(n)
}

/// The participant list, cleaned and deduplicated (order kept).
pub fn clean_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names.iter().filter_map(|n| clean_name(n)) {
        if out.len() >= MAX_PARTICIPANTS {
            break;
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// Host of an `http(s)://` page URL: lowercase letters, digits, dots and
/// dashes only (it becomes part of the frontmatter).
pub fn page_host(url: &str) -> Option<String> {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    let ok = !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    ok.then_some(host)
}

/// Validate a `start`.
pub fn validate_start(info: &StartInfo) -> Result<Start, ProtocolError> {
    if !(MIN_RATE..=MAX_RATE).contains(&info.rate) {
        return Err(bad(format!(
            "rate {} Hz out of range ({MIN_RATE}–{MAX_RATE})",
            info.rate
        )));
    }
    if !(1..=2).contains(&info.channels) {
        return Err(bad(format!("channels must be 1 or 2, got {}", info.channels)));
    }
    let host = page_host(&info.url).ok_or_else(|| bad("url must be the meeting page's http(s) address"))?;
    let platform = match info.platform.trim().to_ascii_lowercase().as_str() {
        p @ ("meet" | "teams" | "zoom") => p.to_string(),
        _ => "other".to_string(),
    };
    Ok(Start {
        title: one_line(&info.title, MAX_TITLE_CHARS),
        host,
        platform,
        rate: info.rate,
        channels: info.channels,
    })
}

/// Validate a `speaker_active` time: milliseconds, finite, not negative.
pub fn validate_t(t: f64) -> Result<u64, ProtocolError> {
    if !t.is_finite() || t < 0.0 {
        return Err(bad("t must be a non-negative number of milliseconds"));
    }
    Ok(t.min(u64::MAX as f64) as u64)
}

/// A binary audio frame, PCM converted to f32 in `[-1, 1)`.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub channel: Channel,
    pub seq: u32,
    pub pcm: Vec<f32>,
}

/// Wire value → channel.
pub fn channel_from_byte(b: u8) -> Option<Channel> {
    match b {
        0 => Some(Channel::Mic),
        1 => Some(Channel::Remote),
        _ => None,
    }
}

/// Parse `[u8 channel][u32 seq LE][i16 pcm LE…]`.
pub fn parse_audio_frame(bytes: &[u8]) -> Result<AudioFrame, ProtocolError> {
    if bytes.len() < FRAME_HEADER {
        return Err(bad(format!(
            "audio frame too short ({} bytes, header is {FRAME_HEADER})",
            bytes.len()
        )));
    }
    let channel =
        channel_from_byte(bytes[0]).ok_or_else(|| bad(format!("unknown channel {}", bytes[0])))?;
    let seq = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    let pcm = &bytes[FRAME_HEADER..];
    if !pcm.len().is_multiple_of(2) {
        return Err(bad("audio frame with an odd number of PCM bytes"));
    }
    let pcm = pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0)
        .collect();
    Ok(AudioFrame { channel, seq, pcm })
}

/// Build an audio frame (the extension's side; tests and tools).
pub fn encode_audio_frame(channel: u8, seq: u32, pcm: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER + pcm.len() * 2);
    out.push(channel);
    out.extend_from_slice(&seq.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

// ---- app → client ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    New,
    /// An edit or a speaker attribution changed a segment already sent.
    Updated,
}

/// A segment as the side panel shows it (no raw text, no word timings).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveSegment {
    pub id: u32,
    pub channel: Channel,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    /// The engine could not transcribe this stretch.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
}

impl From<&Segment> for LiveSegment {
    fn from(s: &Segment) -> Self {
        Self {
            id: s.id,
            channel: s.channel,
            start_ms: s.start_ms,
            end_ms: s.end_ms,
            text: s.text.clone(),
            speaker_id: s.speaker_id.clone(),
            failed: s.stt_error.is_some(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ready,
    Started,
    Recording,
    Finishing,
    Done,
    Warning,
    Error,
}

/// `status`: every field but `state` is optional.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// Seconds of audio received but not transcribed yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backlog_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processed_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_len: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Status {
    pub fn new(state: State) -> Self {
        Self {
            state,
            protocol: None,
            app: None,
            session_id: None,
            item_id: None,
            backlog_s: None,
            processed_s: None,
            queue_len: None,
            message: None,
        }
    }

    pub fn ready() -> Self {
        Self {
            protocol: Some(PROTOCOL_VERSION),
            app: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..Self::new(State::Ready)
        }
    }

    pub fn message(state: State, message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::new(state)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Segment {
        kind: SegmentKind,
        segment: LiveSegment,
    },
    Speaker {
        id: String,
        label: String,
    },
    Status(Status),
}

impl ServerMessage {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| r#"{"type":"status","state":"error"}"#.into())
    }
}

/// Engine event → message for the client (`None`: nothing to tell).
/// `stopping`: the client sent `stop`, so progress reads `finishing`.
pub fn from_engine(event: &crate::engine::EngineEvent, stopping: bool) -> Option<ServerMessage> {
    use crate::engine::EngineEvent as E;
    let status = |state| Status::new(state);
    Some(match event {
        E::Download(_) | E::Warning(_) => return None,
        E::Started(p) => ServerMessage::Status(Status {
            session_id: Some(p.session_id),
            item_id: Some(p.item_id.clone()),
            ..status(State::Started)
        }),
        E::Progress(p) => ServerMessage::Status(Status {
            session_id: Some(p.session_id),
            backlog_s: Some(p.backlog_s),
            processed_s: Some(p.processed_s),
            queue_len: Some(p.queue_len),
            ..status(if stopping {
                State::Finishing
            } else {
                State::Recording
            })
        }),
        E::Segment(p) => ServerMessage::Segment {
            kind: SegmentKind::New,
            segment: LiveSegment::from(&p.segment),
        },
        E::Done(p) => ServerMessage::Status(Status {
            session_id: Some(p.session_id),
            item_id: Some(p.item_id.clone()),
            ..status(State::Done)
        }),
        E::Error(p) => ServerMessage::Status(Status {
            session_id: Some(p.session_id),
            item_id: p.item_id.clone(),
            message: Some(p.error.clone()),
            ..status(State::Error)
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(rate: u32, channels: u8, url: &str) -> StartInfo {
        StartInfo {
            title: "  Weekly\nsync ".into(),
            url: url.into(),
            platform: "Meet".into(),
            rate,
            channels,
        }
    }

    #[test]
    fn control_messages_parse() {
        let m = parse_control(
            r#"{"type":"start","title":"Sync","url":"https://meet.google.com/abc-defg-hij","platform":"meet","rate":48000,"channels":2}"#,
        )
        .unwrap();
        let ClientMessage::Start(s) = m else {
            panic!("not a start: {m:?}")
        };
        assert_eq!((s.rate, s.channels), (48_000, 2));
        assert_eq!(
            parse_control(r#"{"type":"speaker_active","name":"Anna","t":1250.5}"#).unwrap(),
            ClientMessage::SpeakerActive {
                name: "Anna".into(),
                t: 1250.5
            }
        );
        assert_eq!(
            parse_control(r#"{"type":"participants","names":["Anna","Bo"]}"#).unwrap(),
            ClientMessage::Participants {
                names: vec!["Anna".into(), "Bo".into()]
            }
        );
        assert_eq!(parse_control(r#"{"type":"stop"}"#).unwrap(), ClientMessage::Stop);
        assert_eq!(parse_control(r#"{"type":"ping","extra":1}"#).unwrap(), ClientMessage::Ping);
    }

    #[test]
    fn bad_control_messages_are_errors() {
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"type":"dance"}"#,
            r#"{"type":"start","rate":48000}"#,
            r#"{"type":"speaker_active","name":"x"}"#,
            r#"{"type":"participants","names":"Anna"}"#,
        ] {
            assert!(parse_control(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn start_is_validated() {
        let s = validate_start(&info(48_000, 2, "https://Meet.Google.com/abc-defg-hij?authuser=0")).unwrap();
        assert_eq!(s.title, "Weekly sync");
        assert_eq!(s.host, "meet.google.com");
        assert_eq!(s.platform, "meet");
        assert_eq!(s.source_label(), "browser:meet.google.com");
        let mut other = info(16_000, 1, "http://localhost:8080/room");
        other.platform = "jitsi".into();
        let s = validate_start(&other).unwrap();
        assert_eq!((s.host.as_str(), s.platform.as_str()), ("localhost", "other"));
        let long = StartInfo {
            title: "x".repeat(1000),
            ..info(48_000, 2, "https://teams.microsoft.com/")
        };
        assert_eq!(validate_start(&long).unwrap().title.chars().count(), MAX_TITLE_CHARS);

        assert!(validate_start(&info(7_999, 2, "https://a.b")).is_err());
        assert!(validate_start(&info(192_001, 2, "https://a.b")).is_err());
        assert!(validate_start(&info(48_000, 0, "https://a.b")).is_err());
        assert!(validate_start(&info(48_000, 3, "https://a.b")).is_err());
        for url in ["", "meet.google.com", "file:///etc/passwd", "https://", "https://a b.com"] {
            assert!(validate_start(&info(48_000, 2, url)).is_err(), "{url}");
        }
        // Only the host is kept: userinfo, port and path go.
        let s = validate_start(&info(48_000, 2, "https://u:p@x.example:8443/../a")).unwrap();
        assert_eq!(s.host, "x.example");
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Anna \n Rossi "), Some("Anna Rossi".into()));
        assert_eq!(clean_name(" \t"), None);
        assert_eq!(clean_name(&"n".repeat(500)).unwrap().len(), MAX_NAME_CHARS);
        assert_eq!(
            clean_names(&["Anna".into(), " ".into(), "Bo".into(), "Anna".into()]),
            vec!["Anna".to_string(), "Bo".to_string()]
        );
        let many: Vec<String> = (0..2 * MAX_PARTICIPANTS).map(|i| format!("p{i}")).collect();
        assert_eq!(clean_names(&many).len(), MAX_PARTICIPANTS);
        assert_eq!(validate_t(12.9), Ok(12));
        assert!(validate_t(-1.0).is_err());
        assert!(validate_t(f64::NAN).is_err());
        assert!(validate_t(f64::INFINITY).is_err());
    }

    #[test]
    fn audio_frames_round_trip() {
        let bytes = encode_audio_frame(1, 0x0102_0304, &[0, 16_384, -32_768, 32_767]);
        assert_eq!(&bytes[..5], &[1, 4, 3, 2, 1], "channel then seq, little endian");
        let f = parse_audio_frame(&bytes).unwrap();
        assert_eq!(f.channel, Channel::Remote);
        assert_eq!(f.seq, 0x0102_0304);
        assert_eq!(f.pcm, vec![0.0, 0.5, -1.0, 32_767.0 / 32_768.0]);
        let mic = parse_audio_frame(&encode_audio_frame(0, 0, &[])).unwrap();
        assert_eq!((mic.channel, mic.pcm.len()), (Channel::Mic, 0));
    }

    #[test]
    fn bad_audio_frames_are_errors() {
        assert!(parse_audio_frame(&[]).is_err());
        assert!(parse_audio_frame(&[0, 0, 0, 0]).is_err(), "short header");
        assert!(parse_audio_frame(&encode_audio_frame(2, 0, &[1])).is_err(), "channel 2");
        assert!(parse_audio_frame(&encode_audio_frame(255, 0, &[1])).is_err());
        let mut odd = encode_audio_frame(0, 0, &[1, 2]);
        odd.push(7);
        assert!(parse_audio_frame(&odd).is_err(), "odd PCM bytes");
    }

    #[test]
    fn server_messages_serialize_with_a_type() {
        let ready = ServerMessage::Status(Status::ready()).to_json();
        let v: serde_json::Value = serde_json::from_str(&ready).unwrap();
        assert_eq!(v["type"], "status");
        assert_eq!(v["state"], "ready");
        assert_eq!(v["protocol"], PROTOCOL_VERSION);
        assert!(v.get("item_id").is_none(), "unset fields are omitted");
        let seg = Segment {
            id: 3,
            channel: Channel::Remote,
            start_ms: 1_000,
            end_ms: 2_500,
            raw: "ciao".into(),
            text: "Ciao.".into(),
            ..Default::default()
        };
        let msg = ServerMessage::Segment {
            kind: SegmentKind::New,
            segment: LiveSegment::from(&seg),
        };
        let v: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
        assert_eq!(v["type"], "segment");
        assert_eq!(v["kind"], "new");
        assert_eq!(v["segment"]["channel"], "remote");
        assert_eq!(v["segment"]["text"], "Ciao.");
        assert!(v["segment"].get("raw").is_none());
        let sp = ServerMessage::Speaker {
            id: "meet:Anna".into(),
            label: "Anna".into(),
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&sp.to_json()).unwrap()["type"],
            "speaker"
        );
    }

    #[test]
    fn engine_events_map_to_client_messages() {
        use crate::engine::{EngineEvent, ErrorPayload, ProgressPayload};
        let progress = EngineEvent::Progress(ProgressPayload {
            session_id: 4,
            processed_s: 10.0,
            ingested_s: 12.0,
            total_s: 12.0,
            backlog_s: 2.0,
            queue_len: 1,
            segments_done: 3,
        });
        let Some(ServerMessage::Status(s)) = from_engine(&progress, false) else {
            panic!()
        };
        assert_eq!((s.state, s.backlog_s), (State::Recording, Some(2.0)));
        let Some(ServerMessage::Status(s)) = from_engine(&progress, true) else {
            panic!()
        };
        assert_eq!(s.state, State::Finishing);
        let err = EngineEvent::Error(ErrorPayload {
            session_id: 4,
            error: "no speech found in the audio".into(),
            item_id: None,
        });
        let Some(ServerMessage::Status(s)) = from_engine(&err, true) else {
            panic!()
        };
        assert_eq!(s.state, State::Error);
        assert_eq!(s.message.as_deref(), Some("no speech found in the audio"));
    }
}
