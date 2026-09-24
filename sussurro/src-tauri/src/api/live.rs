//! One `/live` WebSocket connection (#126): the browser extension's meeting
//! session. Protocol in [`super::protocol`].
//!
//! **Transport (E5).** The socket is a `tiny_http` request upgraded with
//! [`tiny_http::Request::upgrade`] and framed by `tungstenite`. The upgraded
//! stream is one blocking `Read + Write` object that cannot be split or
//! given a read timeout, so this thread both reads and writes: after every
//! client message it forwards whatever the engine produced meanwhile
//! (segments, status). A live meeting streams audio every few tens of
//! milliseconds, so replies go out promptly; a client with nothing to send
//! can send `ping`. After `stop` the thread only writes: it forwards the
//! rest of the run until the item is written, then closes. This keeps the
//! existing server and its routes unchanged; `axum` + `tokio` (the plan's
//! fallback) would only be needed for server-initiated messages on an idle
//! connection, which the protocol does not require.
//!
//! **Session.** `start` creates a [`BrowserSource`] and asks the [`Host`]
//! to run the long-form engine on it as a `meeting` item
//! (`source: browser:<host>`). Audio frames go through the [`ChannelMux`]
//! into the source; `speaker_active` / `participants` are appended to the
//! item's meeting events (for speaker attribution, #131). A connection that
//! drops without `stop` ends the session the same way — the recording is
//! kept, never discarded. Nothing here logs payloads or the token.

use super::protocol::{self, ClientMessage, ServerMessage, State, Status};
use super::Host;
use crate::archive::meeting::{self, MeetingEvent};
use crate::engine::{EngineEvent, EngineSink};
use crate::sources::browser::{BrowserSource, ChannelMux, Chunk};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use tungstenite::{Message, WebSocket};

/// Warnings sent per connection at most (a broken client must not turn
/// every frame into a reply).
pub const MAX_WARNINGS: usize = 20;

/// What [`Host::start_meeting`] gets: the validated `start`, the source to
/// run, and a sink that forwards the run's events to this connection.
pub struct MeetingStart {
    pub start: protocol::Start,
    pub source: BrowserSource,
    pub sink: Arc<dyn EngineSink>,
}

/// Engine events for this connection.
struct ChannelSink(Mutex<Sender<EngineEvent>>);

impl EngineSink for ChannelSink {
    fn emit(&self, event: &EngineEvent) {
        let _ = self.0.lock().unwrap_or_else(|e| e.into_inner()).send(event.clone());
    }
}

/// tungstenite's settings for this protocol: bounded messages.
pub fn ws_config() -> tungstenite::protocol::WebSocketConfig {
    tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(protocol::MAX_MESSAGE_BYTES))
        .max_frame_size(Some(protocol::MAX_MESSAGE_BYTES))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    /// `stop`: finish the meeting, then close.
    Stop,
    /// The client closed the socket.
    Closed,
}

struct Live {
    session_id: u64,
    mux: ChannelMux,
    /// `None` once the audio has ended (stop) or the run stopped taking it.
    tx: Option<SyncSender<Chunk>>,
    archive: Option<PathBuf>,
    item_id: Option<String>,
    /// Events received before the item id was known.
    pending: Vec<MeetingEvent>,
}

impl Live {
    fn record(&mut self, event: MeetingEvent) {
        self.pending.push(event);
        self.flush_events();
    }

    fn flush_events(&mut self) {
        let (Some(archive), Some(id)) = (&self.archive, &self.item_id) else {
            return;
        };
        if self.pending.is_empty() {
            return;
        }
        let events = std::mem::take(&mut self.pending);
        if let Err(e) = meeting::append_events(archive, id, &events) {
            eprintln!("live: meeting events not saved ({e:#})");
        }
    }
}

/// The connection's state machine, independent of the socket (tests drive
/// it with messages).
struct Conn<'a> {
    host: &'a dyn Host,
    live: Option<Live>,
    events_tx: Option<Sender<EngineEvent>>,
    events_rx: Receiver<EngineEvent>,
    stopping: bool,
    /// The run finished (done or error).
    ended: bool,
    warnings: usize,
}

impl<'a> Conn<'a> {
    fn new(host: &'a dyn Host) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            host,
            live: None,
            events_tx: Some(tx),
            events_rx: rx,
            stopping: false,
            ended: false,
            warnings: 0,
        }
    }

    fn warn(&mut self, out: &mut Vec<ServerMessage>, message: impl Into<String>) {
        if self.warnings < MAX_WARNINGS {
            self.warnings += 1;
            out.push(ServerMessage::Status(Status::message(State::Warning, message)));
        }
    }

    fn status_now(&self) -> Status {
        let state = match (&self.live, self.stopping, self.ended) {
            (None, _, _) => State::Ready,
            (Some(_), _, true) => State::Done,
            (Some(_), true, false) => State::Finishing,
            (Some(_), false, false) => State::Recording,
        };
        Status {
            session_id: self.live.as_ref().map(|l| l.session_id),
            item_id: self.live.as_ref().and_then(|l| l.item_id.clone()),
            ..Status::new(state)
        }
    }

    fn on_message(&mut self, msg: Message, out: &mut Vec<ServerMessage>) -> Flow {
        match msg {
            Message::Text(text) => match protocol::parse_control(text.as_str()) {
                Ok(m) => self.on_control(m, out),
                Err(e) => {
                    self.warn(out, e.to_string());
                    Flow::Continue
                }
            },
            Message::Binary(bytes) => {
                self.on_audio(&bytes, out);
                Flow::Continue
            }
            Message::Close(_) => Flow::Closed,
            // tungstenite answers pings itself.
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Flow::Continue,
        }
    }

    fn on_control(&mut self, msg: ClientMessage, out: &mut Vec<ServerMessage>) -> Flow {
        match msg {
            ClientMessage::Start(info) => {
                if self.live.is_some() {
                    self.warn(out, "a meeting is already running on this connection");
                    return Flow::Continue;
                }
                let start = match protocol::validate_start(&info) {
                    Ok(s) => s,
                    Err(e) => {
                        out.push(ServerMessage::Status(Status::message(State::Error, e.to_string())));
                        return Flow::Continue;
                    }
                };
                let Some(events_tx) = self.events_tx.clone() else {
                    return Flow::Continue;
                };
                let (tx, source) = BrowserSource::channel();
                let mux = ChannelMux::new(start.rate);
                let archive = self.host.archive_dir().ok();
                let sink: Arc<dyn EngineSink> = Arc::new(ChannelSink(Mutex::new(events_tx)));
                match self.host.start_meeting(MeetingStart {
                    start,
                    source,
                    sink,
                }) {
                    Ok(session_id) => {
                        self.live = Some(Live {
                            session_id,
                            mux,
                            tx: Some(tx),
                            archive,
                            item_id: None,
                            pending: Vec::new(),
                        });
                    }
                    Err(e) => out.push(ServerMessage::Status(Status::message(
                        State::Error,
                        format!("{e:#}"),
                    ))),
                }
                Flow::Continue
            }
            ClientMessage::SpeakerActive { name, t } => {
                let Some(live) = self.live.as_mut() else {
                    self.warn(out, "speaker_active before start");
                    return Flow::Continue;
                };
                match (protocol::validate_t(t), protocol::clean_name(&name)) {
                    (Ok(t_ms), Some(name)) => {
                        let at_ms = live.mux.position_ms();
                        live.record(MeetingEvent::SpeakerActive { at_ms, t_ms, name });
                    }
                    (Err(e), _) => self.warn(out, e.to_string()),
                    (_, None) => self.warn(out, "speaker_active without a name"),
                }
                Flow::Continue
            }
            ClientMessage::Participants { names } => {
                let Some(live) = self.live.as_mut() else {
                    self.warn(out, "participants before start");
                    return Flow::Continue;
                };
                let at_ms = live.mux.position_ms();
                live.record(MeetingEvent::Participants {
                    at_ms,
                    names: protocol::clean_names(&names),
                });
                Flow::Continue
            }
            ClientMessage::Stop => Flow::Stop,
            ClientMessage::Ping => {
                out.push(ServerMessage::Status(self.status_now()));
                Flow::Continue
            }
        }
    }

    fn on_audio(&mut self, bytes: &[u8], out: &mut Vec<ServerMessage>) {
        let Some(live) = self.live.as_mut() else {
            self.warn(out, "audio before start");
            return;
        };
        let frame = match protocol::parse_audio_frame(bytes) {
            Ok(f) => f,
            Err(e) => {
                self.warn(out, e.to_string());
                return;
            }
        };
        let pushed = live.mux.push(frame.channel, frame.seq, &frame.pcm);
        if let Some(tx) = &live.tx {
            for chunk in pushed.chunks {
                if tx.send(chunk).is_err() {
                    // The run ended (failed or cancelled from the app).
                    live.tx = None;
                    break;
                }
            }
        }
        if pushed.lost > 0 {
            let ch = serde_json::to_value(frame.channel)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            self.warn(
                out,
                format!("{} {ch} audio frame(s) lost, filled with silence", pushed.lost),
            );
        }
    }

    /// Engine events → client messages; `wait` blocks for the next one.
    fn engine_messages(&mut self, wait: bool) -> Option<Vec<ServerMessage>> {
        let first = if wait {
            self.events_rx.recv().ok()?
        } else {
            match self.events_rx.try_recv() {
                Ok(e) => e,
                Err(_) => return Some(Vec::new()),
            }
        };
        let mut out = Vec::new();
        let mut next = Some(first);
        while let Some(event) = next {
            self.on_engine(&event);
            if let Some(m) = protocol::from_engine(&event, self.stopping) {
                out.push(m);
            }
            next = self.events_rx.try_recv().ok();
        }
        Some(out)
    }

    fn on_engine(&mut self, event: &EngineEvent) {
        let Some(live) = self.live.as_mut() else {
            return;
        };
        match event {
            EngineEvent::Started(p) => {
                live.item_id = Some(p.item_id.clone());
                live.flush_events();
            }
            EngineEvent::Done(p) => {
                // The folder may have been renamed after the final title.
                live.item_id = Some(p.item_id.clone());
                live.flush_events();
                self.ended = true;
            }
            EngineEvent::Error(p) => {
                if let Some(kept) = &p.item_id {
                    live.item_id = Some(kept.clone());
                    live.flush_events();
                }
                self.ended = true;
            }
            _ => {}
        }
    }

    /// No more audio: flush the resamplers into the source and close it,
    /// so the engine finishes the item. Also drops this connection's own
    /// event sender, so waiting for events ends with the run.
    fn end_audio(&mut self) {
        self.stopping = true;
        self.events_tx = None;
        if let Some(live) = self.live.as_mut() {
            if let Some(tx) = live.tx.take() {
                for chunk in live.mux.finish() {
                    if tx.send(chunk).is_err() {
                        break;
                    }
                }
            }
        }
    }
}

fn send<S: Read + Write>(ws: &mut WebSocket<S>, msg: &ServerMessage) -> bool {
    ws.send(Message::text(msg.to_json())).is_ok()
}

/// Serve one upgraded connection until the meeting (if any) is written.
pub fn serve<S: Read + Write>(mut ws: WebSocket<S>, host: &dyn Host) {
    let mut conn = Conn::new(host);
    let mut open = send(&mut ws, &ServerMessage::Status(Status::ready()));
    while open {
        let msg = match ws.read() {
            Ok(m) => m,
            // Closed, reset, or a protocol violation (oversized message…):
            // either way nothing more can be read or written.
            Err(_) => {
                open = false;
                break;
            }
        };
        let mut out = Vec::new();
        let flow = conn.on_message(msg, &mut out);
        if flow == Flow::Closed {
            // tungstenite queued the close reply; send it, then only finish.
            let _ = ws.flush();
            open = false;
            break;
        }
        if flow == Flow::Stop {
            conn.stopping = true;
        }
        out.extend(conn.engine_messages(false).unwrap_or_default());
        for m in &out {
            if !send(&mut ws, m) {
                open = false;
                break;
            }
        }
        if flow == Flow::Stop {
            break;
        }
    }
    conn.end_audio();
    // Forward the rest of the run (or, with the client gone, just wait for
    // it so late meeting events land in the right folder).
    while let Some(msgs) = conn.engine_messages(true) {
        if open {
            for m in &msgs {
                if !send(&mut ws, m) {
                    open = false;
                    break;
                }
            }
        }
        if conn.ended {
            break;
        }
    }
    if open {
        let _ = ws.close(None);
        let _ = ws.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiConfig;

    /// A host whose meetings never start (the state machine alone).
    struct NoMeetings;

    impl Host for NoMeetings {
        fn config(&self) -> ApiConfig {
            ApiConfig::default()
        }
        fn clean(&self, _: &str) -> serde_json::Value {
            serde_json::Value::Null
        }
        fn transcribe(&self, _: Vec<u8>, _: &str) -> (u16, serde_json::Value) {
            (500, serde_json::Value::Null)
        }
        fn history(&self, _: &str, _: usize) -> serde_json::Value {
            serde_json::Value::Null
        }
        fn archive_dir(&self) -> anyhow::Result<PathBuf> {
            anyhow::bail!("no archive")
        }
        fn open_item(&self, _: &str) -> anyhow::Result<()> {
            anyhow::bail!("no")
        }
        fn start_meeting(&self, _: MeetingStart) -> anyhow::Result<u64> {
            anyhow::bail!("a meeting is already being recorded")
        }
    }

    fn states(out: &[ServerMessage]) -> Vec<State> {
        out.iter()
            .filter_map(|m| match m {
                ServerMessage::Status(s) => Some(s.state),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn messages_before_start_are_warnings_and_a_failed_start_is_an_error() {
        let host = NoMeetings;
        let mut c = Conn::new(&host);
        let mut out = Vec::new();
        let audio = protocol::encode_audio_frame(0, 0, &[1, 2, 3]);
        assert_eq!(c.on_message(Message::binary(audio), &mut out), Flow::Continue);
        c.on_message(Message::text(r#"{"type":"speaker_active","name":"A","t":1}"#), &mut out);
        c.on_message(Message::text("garbage"), &mut out);
        assert_eq!(states(&out), [State::Warning; 3]);
        out.clear();
        c.on_message(Message::text(r#"{"type":"ping"}"#), &mut out);
        assert_eq!(states(&out), [State::Ready]);
        out.clear();
        // Invalid start, then a start the host refuses: errors, still open.
        c.on_message(
            Message::text(r#"{"type":"start","url":"x","rate":48000,"channels":2}"#),
            &mut out,
        );
        c.on_message(
            Message::text(
                r#"{"type":"start","url":"https://meet.google.com/a","rate":48000,"channels":2}"#,
            ),
            &mut out,
        );
        assert_eq!(states(&out), [State::Error, State::Error]);
        let ServerMessage::Status(s) = &out[1] else { panic!() };
        assert!(s.message.as_deref().unwrap().contains("already"));
        assert!(c.live.is_none());
        assert_eq!(c.on_message(Message::text(r#"{"type":"stop"}"#), &mut out), Flow::Stop);
        assert_eq!(c.on_message(Message::Close(None), &mut out), Flow::Closed);
    }

    #[test]
    fn warnings_are_capped() {
        let host = NoMeetings;
        let mut c = Conn::new(&host);
        let mut out = Vec::new();
        for _ in 0..3 * MAX_WARNINGS {
            c.on_message(Message::binary(vec![9u8]), &mut out);
        }
        assert_eq!(out.len(), MAX_WARNINGS);
    }

    #[test]
    fn with_no_meeting_waiting_for_events_ends_at_once() {
        let host = NoMeetings;
        let mut c = Conn::new(&host);
        c.end_audio();
        assert!(c.engine_messages(true).is_none());
    }
}
