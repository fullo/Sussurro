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
//! into the source; the page's events (`speaker_active`, `speaker_idle`,
//! `speaker_name`, `participants`, `observer_health`) are appended to the
//! item's meeting events and fed to the run's name timeline
//! ([`SharedNames`], speaker attribution #131). The first segment of each
//! speaker is preceded by a `speaker {id, label}`. A connection that
//! drops without `stop` ends the session the same way — the recording is
//! kept, never discarded. Nothing here logs payloads or the token.
//!
//! **Auth.** The upgrade carries the token as `?token=` (protocol 1–2
//! clients), or none: then the first message must be `auth {token}`,
//! within [`AUTH_TIMEOUT`] of the upgrade ([`authorize`], #217), so the
//! token stays out of URLs the browser may log. The upgraded stream cannot
//! time out a read, so a client that sends nothing keeps its thread parked
//! until the peer goes away (as any idle socket on the server); one that
//! answers late, or with anything but the right token, is closed.
//!
//! **Limits (#217).** The page's events come from a web page (or an XSS on
//! it) through the extension: per connection they are rate-limited
//! ([`EVENTS_BURST`], [`EVENTS_PER_SEC`]) and capped per meeting
//! ([`MAX_PAGE_EVENTS`]); a `participants` list equal to the last one is
//! dropped, and all the lists of a meeting may take at most
//! [`MAX_PARTICIPANTS_BYTES`]. The events file stays open while the
//! meeting records. Audio `seq` gaps insert at most the wall-clock time
//! since `start` of silence ([`ChannelMux::push`]).

use super::auth::{self, Denied};
use super::protocol::{self, ClientMessage, ServerMessage, State, Status};
use super::Host;
use crate::archive::meeting::{EventsWriter, MeetingEvent};
use crate::engine::{EngineEvent, EngineSink};
use crate::sources::browser::{BrowserSource, ChannelMux, Chunk};
use crate::speakers::names::{AttributionParams, SharedNames};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::CloseFrame;
use tungstenite::{Message, WebSocket};

/// Warnings sent per connection at most (a broken client must not turn
/// every frame into a reply).
pub const MAX_WARNINGS: usize = 20;
/// Page events (`speaker_*`, `participants`, `observer_health`) accepted
/// in a burst, e.g. the state a new connection replays after `start`…
pub const EVENTS_BURST: f64 = 400.0;
/// …and per second after it (a busy call sends a few).
pub const EVENTS_PER_SEC: f64 = 20.0;
/// Page events recorded per meeting at most (~15 MB of events file).
pub const MAX_PAGE_EVENTS: usize = 100_000;
/// Bytes of participant names a meeting records at most over all its
/// `participants` lists (a 500-name list is ~10 KB).
pub const MAX_PARTICIPANTS_BYTES: usize = 8 * 1024 * 1024;
/// Time the first message has to authenticate a connection opened
/// without `?token=`.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(2);

/// How a `/live` upgrade is authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveAuth {
    /// The URL's `?token=` was checked before the upgrade.
    Done,
    /// No token in the URL: the first message must be `auth {token}`
    /// before `deadline`.
    FirstMessage { expected: String, deadline: Instant },
}

/// A `/live` upgrade request: with `?token=`, checked now
/// ([`auth::check_ws`]); without, an extension origin and a paired app are
/// required now and the token comes in the first message.
pub fn authorize(
    expected: &str,
    query_token: Option<&str>,
    origin: Option<&str>,
) -> Result<LiveAuth, Denied> {
    if query_token.is_some() {
        return auth::check_ws(expected, query_token, origin).map(|()| LiveAuth::Done);
    }
    if !origin.is_some_and(auth::is_extension_origin) {
        return Err(Denied::Forbidden);
    }
    let expected = expected.trim();
    if expected.is_empty() {
        return Err(Denied::Unauthorized);
    }
    Ok(LiveAuth::FirstMessage {
        expected: expected.to_string(),
        deadline: Instant::now() + AUTH_TIMEOUT,
    })
}

#[derive(serde::Deserialize)]
struct AuthMessage {
    #[serde(rename = "type")]
    kind: String,
    token: String,
}

/// Pure: does `msg`, the connection's first, authenticate it? `late`: it
/// came after the deadline.
fn check_auth_message(msg: &Message, expected: &str, late: bool) -> Result<(), &'static str> {
    if late {
        return Err("authentication timed out");
    }
    let Message::Text(text) = msg else {
        return Err("authentication required");
    };
    match serde_json::from_str::<AuthMessage>(text.as_str()) {
        Ok(a) if a.kind == "auth" && !expected.is_empty() && auth::tokens_match(expected, &a.token) => {
            Ok(())
        }
        Ok(a) if a.kind == "auth" => Err("wrong extension token"),
        _ => Err("authentication required"),
    }
}

/// A token bucket over a caller-supplied clock. Pure.
#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    last: Duration,
    burst: f64,
    per_sec: f64,
}

impl Bucket {
    fn new(burst: f64, per_sec: f64) -> Self {
        Self {
            tokens: burst,
            last: Duration::ZERO,
            burst,
            per_sec,
        }
    }

    fn allow(&mut self, now: Duration) -> bool {
        let dt = now.saturating_sub(self.last).as_secs_f64();
        self.last = self.last.max(now);
        self.tokens = (self.tokens + dt * self.per_sec).min(self.burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// What [`Host::start_meeting`] gets: the validated `start`, the source to
/// run, and a sink that forwards the run's events to this connection.
pub struct MeetingStart {
    pub start: protocol::Start,
    pub source: BrowserSource,
    pub sink: Arc<dyn EngineSink>,
    /// The page's speaker timeline, filled by this connection as events
    /// arrive and read by the run to name remote lines (#131).
    pub names: SharedNames,
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
    /// The connection clock's reading at `start`.
    started: Duration,
    mux: ChannelMux,
    /// `None` once the audio has ended (stop) or the run stopped taking it.
    tx: Option<SyncSender<Chunk>>,
    archive: Option<PathBuf>,
    item_id: Option<String>,
    /// Events received before the item id was known.
    pending: Vec<MeetingEvent>,
    /// The same events, for the run's attribution (#131).
    names: SharedNames,
    /// The events file, open while the meeting records.
    writer: Option<EventsWriter>,
    /// The item whose events file could not be opened (not retried).
    unwritable: Option<String>,
    /// Page events recorded so far.
    page_events: usize,
    /// The last `participants` list recorded, and the bytes of all of them.
    last_participants: Option<Vec<String>>,
    participants_bytes: usize,
}

impl Live {
    fn record(&mut self, event: MeetingEvent) {
        self.names.push(&event);
        self.pending.push(event);
        self.page_events += 1;
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
        if self.writer.as_ref().is_some_and(|w| w.id() != id) {
            // The folder was renamed: follow it.
            self.writer = None;
        }
        if self.writer.is_none() {
            if self.unwritable.as_deref() == Some(id.as_str()) {
                return;
            }
            match EventsWriter::open(archive, id) {
                Ok(w) => self.writer = Some(w),
                Err(e) => {
                    eprintln!("live: meeting events not saved ({e:#})");
                    self.unwritable = Some(id.clone());
                    return;
                }
            }
        }
        if let Some(w) = self.writer.as_mut() {
            if let Err(e) = w.append(&events) {
                eprintln!("live: meeting events not saved ({e:#})");
            }
        }
    }

    /// Close the events file (the run may rename or delete the folder now).
    fn close_events(&mut self) {
        self.writer = None;
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
    /// Speakers already announced to the client (`speaker`, #131).
    announced: Vec<String>,
    /// Time since the connection opened (a fake one in tests).
    clock: Box<dyn Fn() -> Duration + 'a>,
    /// Page events per second.
    events: Bucket,
    /// Page events are being dropped (rate or total): warned once per spell.
    throttled: bool,
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
            announced: Vec::new(),
            clock: {
                let t0 = Instant::now();
                Box::new(move || t0.elapsed())
            },
            events: Bucket::new(EVENTS_BURST, EVENTS_PER_SEC),
            throttled: false,
        }
    }

    /// Whether a page event may be recorded now: under the meeting's total
    /// and the connection's rate. A drop warns once per spell.
    fn admit_page_event(&mut self, out: &mut Vec<ServerMessage>) -> bool {
        let now = (self.clock)();
        let full = self.live.as_ref().is_some_and(|l| l.page_events >= MAX_PAGE_EVENTS);
        if !full && self.events.allow(now) {
            self.throttled = false;
            return true;
        }
        if !self.throttled {
            self.throttled = true;
            let why = if full {
                "too many page events for one meeting: ignoring the rest"
            } else {
                "page events too fast: some were dropped"
            };
            self.warn(out, why);
        }
        false
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
        let page_event = matches!(
            msg,
            ClientMessage::SpeakerActive { .. }
                | ClientMessage::SpeakerIdle { .. }
                | ClientMessage::SpeakerName { .. }
                | ClientMessage::ObserverHealth { .. }
                | ClientMessage::Participants { .. }
        );
        if page_event && self.live.is_some() && !self.admit_page_event(out) {
            return Flow::Continue;
        }
        match msg {
            ClientMessage::Start(info) => {
                if self.live.is_some() {
                    self.warn(out, "a meeting is already running on this connection");
                    return Flow::Continue;
                }
                let mut start = match protocol::validate_start(&info) {
                    Ok(s) => s,
                    Err(e) => {
                        out.push(ServerMessage::Status(Status::message(State::Error, e.to_string())));
                        return Flow::Continue;
                    }
                };
                // The meeting's language (#288), checked against the
                // engine the run will use; a bad one never stops the
                // meeting: the dictation's language applies.
                match protocol::meeting_language(info.language.as_deref(), self.host.config().languages) {
                    Ok(language) => start.language = language,
                    Err(e) => self.warn(out, format!("{e}: using the dictation language")),
                }
                let Some(events_tx) = self.events_tx.clone() else {
                    return Flow::Continue;
                };
                let (tx, source) = BrowserSource::channel();
                let mux = ChannelMux::new(start.rate);
                let archive = self.host.archive_dir().ok();
                let sink: Arc<dyn EngineSink> = Arc::new(ChannelSink(Mutex::new(events_tx)));
                let names = SharedNames::new(AttributionParams::default());
                let started = (self.clock)();
                match self.host.start_meeting(MeetingStart {
                    start,
                    source,
                    sink,
                    names: names.clone(),
                }) {
                    Ok(session_id) => {
                        self.live = Some(Live {
                            session_id,
                            started,
                            mux,
                            tx: Some(tx),
                            archive,
                            item_id: None,
                            pending: Vec::new(),
                            names,
                            writer: None,
                            unwritable: None,
                            page_events: 0,
                            last_participants: None,
                            participants_bytes: 0,
                        });
                    }
                    Err(e) => out.push(ServerMessage::Status(Status::message(
                        State::Error,
                        format!("{e:#}"),
                    ))),
                }
                Flow::Continue
            }
            ClientMessage::SpeakerActive { name, id, t, source } => {
                let Some(live) = self.live.as_mut() else {
                    self.warn(out, "speaker_active before start");
                    return Flow::Continue;
                };
                match (
                    protocol::validate_t(t),
                    protocol::clean_speaker(id.as_deref(), name.as_deref()),
                ) {
                    (Ok(t_ms), Ok((id, name))) => {
                        let at_ms = live.mux.position_ms();
                        live.record(MeetingEvent::SpeakerActive {
                            at_ms,
                            t_ms,
                            name,
                            id,
                            source: protocol::clean_source(source.as_deref()),
                        });
                    }
                    (Err(e), _) | (_, Err(e)) => self.warn(out, format!("speaker_active: {e}")),
                }
                Flow::Continue
            }
            ClientMessage::SpeakerIdle { name, id, t } => {
                let Some(live) = self.live.as_mut() else {
                    self.warn(out, "speaker_idle before start");
                    return Flow::Continue;
                };
                match (
                    protocol::validate_t(t),
                    protocol::clean_speaker(id.as_deref(), name.as_deref()),
                ) {
                    (Ok(t_ms), Ok((id, name))) => {
                        let at_ms = live.mux.position_ms();
                        live.record(MeetingEvent::SpeakerIdle {
                            at_ms,
                            t_ms,
                            name,
                            id,
                        });
                    }
                    (Err(e), _) | (_, Err(e)) => self.warn(out, format!("speaker_idle: {e}")),
                }
                Flow::Continue
            }
            ClientMessage::SpeakerName { id, name } => {
                let Some(live) = self.live.as_mut() else {
                    self.warn(out, "speaker_name before start");
                    return Flow::Continue;
                };
                match protocol::clean_id(&id) {
                    Some(id) => {
                        let at_ms = live.mux.position_ms();
                        live.record(MeetingEvent::SpeakerName {
                            at_ms,
                            id,
                            name: name.as_deref().and_then(protocol::clean_name),
                        });
                    }
                    None => self.warn(out, "speaker_name: bad speaker id"),
                }
                Flow::Continue
            }
            ClientMessage::ObserverHealth { state, set, hooks } => {
                let Some(live) = self.live.as_mut() else {
                    self.warn(out, "observer_health before start");
                    return Flow::Continue;
                };
                let (state, set, hooks) = protocol::clean_health(&state, set.as_deref(), &hooks);
                let at_ms = live.mux.position_ms();
                live.record(MeetingEvent::ObserverHealth {
                    at_ms,
                    state,
                    set,
                    hooks,
                });
                Flow::Continue
            }
            ClientMessage::Participants { names } => {
                let Some(live) = self.live.as_mut() else {
                    self.warn(out, "participants before start");
                    return Flow::Continue;
                };
                let names = protocol::clean_names(&names);
                if live.last_participants.as_ref() == Some(&names) {
                    // The same list again: nothing new to record.
                    return Flow::Continue;
                }
                let bytes: usize = names.iter().map(|n| n.len() + 1).sum();
                if live.participants_bytes + bytes > MAX_PARTICIPANTS_BYTES {
                    self.warn(out, "too many participant updates for one meeting: ignoring the rest");
                    return Flow::Continue;
                }
                live.participants_bytes += bytes;
                live.last_participants = Some(names.clone());
                let at_ms = live.mux.position_ms();
                live.record(MeetingEvent::Participants { at_ms, names });
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
        let now = (self.clock)();
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
        let elapsed = now.saturating_sub(live.started);
        let pushed = live.mux.push(frame.channel, frame.seq, &frame.pcm, elapsed);
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
            if let Some(m) = self.announce(&event) {
                out.push(m);
            }
            if let Some(m) = protocol::from_engine(&event, self.stopping) {
                out.push(m);
            }
            next = self.events_rx.try_recv().ok();
        }
        Some(out)
    }

    /// `speaker {id, label, color}` the first time a segment names a
    /// speaker: "You", a name from the page, or "Voice N" (#131), with the
    /// colour the item gives it (names are coloured in order of first
    /// appearance, as the tracker lists them).
    fn announce(&mut self, event: &EngineEvent) -> Option<ServerMessage> {
        use crate::speakers::doc;
        let EngineEvent::Segment(p) = event else {
            return None;
        };
        let id = p.segment.speaker_id.as_deref()?;
        if self.announced.iter().any(|a| a == id) {
            return None;
        }
        let label = doc::default_label(id)?;
        let color = if id == doc::YOU_ID {
            doc::YOU_COLOR.to_string()
        } else if let Some(n) = doc::voice_number(id) {
            doc::voice_color(n)
        } else {
            let k = self.announced.iter().filter(|a| doc::is_meet(a)).count();
            doc::meet_speaker(&label, k).color
        };
        self.announced.push(id.to_string());
        Some(ServerMessage::Speaker {
            id: id.to_string(),
            label,
            color: Some(color),
        })
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
                live.close_events();
                self.ended = true;
            }
            EngineEvent::Error(p) => {
                if let Some(kept) = &p.item_id {
                    live.item_id = Some(kept.clone());
                    live.flush_events();
                }
                live.close_events();
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
            // No more page events: let the run rename or discard the folder.
            live.close_events();
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

/// [`LiveAuth::FirstMessage`]: read the first message and check it; on a
/// failure the client gets an error status and the socket is closed.
fn first_message_auth<S: Read + Write>(ws: &mut WebSocket<S>, expected: &str, deadline: Instant) -> bool {
    let Ok(msg) = ws.read() else {
        return false;
    };
    match check_auth_message(&msg, expected, Instant::now() > deadline) {
        Ok(()) => true,
        Err(why) => {
            let _ = send(ws, &ServerMessage::Status(Status::message(State::Error, why)));
            let _ = ws.close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: "unauthorized".into(),
            }));
            let _ = ws.flush();
            false
        }
    }
}

/// Serve one upgraded connection until the meeting (if any) is written.
pub fn serve<S: Read + Write>(mut ws: WebSocket<S>, host: &dyn Host, auth: LiveAuth) {
    if let LiveAuth::FirstMessage { expected, deadline } = &auth {
        if !first_message_auth(&mut ws, expected, *deadline) {
            return;
        }
    }
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

    impl crate::api::archive_write::NoteHost for NoMeetings {}

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

    const T: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const EXT: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";

    /// A host whose meetings start (the run itself is not needed: the
    /// source is dropped) with an archive in a temp dir.
    struct Meetings(PathBuf);

    impl crate::api::archive_write::NoteHost for Meetings {}

    impl Host for Meetings {
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
            Ok(self.0.clone())
        }
        fn open_item(&self, _: &str) -> anyhow::Result<()> {
            anyhow::bail!("no")
        }
        fn start_meeting(&self, _: MeetingStart) -> anyhow::Result<u64> {
            Ok(7)
        }
    }

    /// A host that keeps the language of each meeting it starts, on an
    /// engine offering `set` (#288).
    struct Languages {
        set: crate::stt::languages::LanguageSet,
        started: Mutex<Vec<Option<String>>>,
    }

    impl crate::api::archive_write::NoteHost for Languages {}

    impl Host for Languages {
        fn config(&self) -> ApiConfig {
            ApiConfig {
                languages: self.set,
                dictation_language: "it".into(),
                ..Default::default()
            }
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
        fn start_meeting(&self, m: MeetingStart) -> anyhow::Result<u64> {
            self.started.lock().unwrap().push(m.start.language);
            Ok(1)
        }
    }

    #[test]
    fn the_start_language_reaches_the_run_or_falls_back_with_a_warning() {
        use crate::stt::languages::LanguageSet;
        let start = |set, extra: &str| {
            let host = Languages {
                set,
                started: Mutex::new(Vec::new()),
            };
            let mut c = Conn::new(&host);
            let mut out = Vec::new();
            c.on_message(
                Message::text(format!(
                    r#"{{"type":"start","url":"https://meet.google.com/a","rate":16000,"channels":2{extra}}}"#
                )),
                &mut out,
            );
            assert!(c.live.is_some(), "the meeting starts in any case: {out:?}");
            let started = host.started.lock().unwrap().clone();
            assert_eq!(started.len(), 1);
            (started[0].clone(), warnings(&out))
        };
        // An older extension: no language, the dictation setting applies.
        assert_eq!(start(LanguageSet::Whisper, ""), (None, vec![]));
        assert_eq!(
            start(LanguageSet::Whisper, r#","language":"en""#),
            (Some("en".into()), vec![])
        );
        assert_eq!(
            start(LanguageSet::Parakeet, r#","language":"auto""#),
            (Some("auto".into()), vec![])
        );
        // Not one the engine offers: warned, then the dictation setting.
        let (lang, warned) = start(LanguageSet::Parakeet, r#","language":"ja""#);
        assert_eq!(lang, None);
        assert_eq!(warned.len(), 1);
        assert!(warned[0].contains("Parakeet") && warned[0].contains("dictation language"), "{warned:?}");
        let (lang, warned) = start(LanguageSet::Whisper, r#","language":"klingon""#);
        assert_eq!((lang, warned.len()), (None, 1));
    }

    /// A connection on a fake clock, with a meeting started at t = 0.
    fn started(host: &Meetings, now: std::rc::Rc<std::cell::Cell<Duration>>) -> Conn<'_> {
        let mut c = Conn::new(host);
        c.clock = Box::new(move || now.get());
        let mut out = Vec::new();
        c.on_message(
            Message::text(r#"{"type":"start","url":"https://meet.google.com/a","rate":16000,"channels":2}"#),
            &mut out,
        );
        assert!(c.live.is_some(), "{out:?}");
        c
    }

    fn warnings(out: &[ServerMessage]) -> Vec<String> {
        out.iter()
            .filter_map(|m| match m {
                ServerMessage::Status(s) if s.state == State::Warning => s.message.clone(),
                _ => None,
            })
            .collect()
    }

    fn meeting_item(archive: &std::path::Path) -> String {
        use crate::archive::types::{ItemMeta, ItemType, SegmentsFile};
        let meta = ItemMeta {
            item_type: ItemType::Meeting,
            title: "Sync".into(),
            date: "2026-09-24T10:00:00+02:00".into(),
            source: "browser:meet.google.com".into(),
            ..Default::default()
        };
        crate::archive::store::create_item(archive, &meta, &SegmentsFile::default()).unwrap()
    }

    #[test]
    fn a_live_upgrade_takes_the_url_token_or_waits_for_the_first_message() {
        assert_eq!(authorize(T, Some(T), Some(EXT)), Ok(LiveAuth::Done));
        assert_eq!(authorize(T, Some("x"), Some(EXT)), Err(Denied::Unauthorized));
        assert_eq!(authorize(T, Some(T), None), Err(Denied::Forbidden));
        let before = Instant::now();
        let Ok(LiveAuth::FirstMessage { expected, deadline }) = authorize(T, None, Some(EXT)) else {
            panic!("no token: first-message auth")
        };
        assert_eq!(expected, T);
        assert!(deadline >= before + AUTH_TIMEOUT && deadline <= Instant::now() + AUTH_TIMEOUT);
        // Still an extension origin and a paired app, before any upgrade.
        assert_eq!(authorize(T, None, None), Err(Denied::Forbidden));
        assert_eq!(authorize(T, None, Some("https://meet.google.com")), Err(Denied::Forbidden));
        assert_eq!(authorize("  ", None, Some(EXT)), Err(Denied::Unauthorized));
    }

    #[test]
    fn the_first_message_must_be_the_right_token_in_time() {
        let auth = |t: &str| Message::text(format!(r#"{{"type":"auth","token":"{t}"}}"#));
        assert_eq!(check_auth_message(&auth(T), T, false), Ok(()));
        assert_eq!(check_auth_message(&auth(T), T, true), Err("authentication timed out"));
        assert_eq!(check_auth_message(&auth("nope"), T, false), Err("wrong extension token"));
        assert_eq!(check_auth_message(&auth(""), "", false), Err("wrong extension token"));
        for other in [
            Message::text(r#"{"type":"ping"}"#),
            Message::text(r#"{"type":"start","url":"x","rate":1,"channels":1}"#),
            Message::text("garbage"),
            Message::binary(vec![0u8; 9]),
        ] {
            assert_eq!(check_auth_message(&other, T, false), Err("authentication required"));
        }
    }

    #[test]
    fn a_token_bucket_allows_a_burst_then_a_rate() {
        let mut b = Bucket::new(3.0, 2.0);
        let at = Duration::from_millis;
        assert!((0..3).all(|_| b.allow(at(0))));
        assert!(!b.allow(at(0)));
        assert!(!b.allow(at(400)));
        assert!(b.allow(at(500)), "one token after 0.5 s at 2/s");
        assert!(!b.allow(at(500)));
        // A long pause refills to the burst, not beyond.
        assert_eq!((0..10).filter(|_| b.allow(at(60_000))).count(), 3);
        // A clock going backwards gives nothing.
        assert!(!b.allow(at(1_000)));
    }

    #[test]
    fn page_events_are_rate_limited_and_capped_per_meeting() {
        let tmp = tempfile::tempdir().unwrap();
        let host = Meetings(tmp.path().join("Sussurro"));
        let now = std::rc::Rc::new(std::cell::Cell::new(Duration::ZERO));
        let mut c = started(&host, now.clone());
        let mut out = Vec::new();
        let name = |i: usize| Message::text(format!(r#"{{"type":"speaker_name","id":"csrc:{i}","name":"P{i}"}}"#));
        for i in 0..1_000 {
            c.on_message(name(i), &mut out);
        }
        let live = c.live.as_ref().unwrap();
        assert_eq!(live.page_events, EVENTS_BURST as usize);
        assert_eq!(warnings(&out), ["page events too fast: some were dropped"], "one warning per spell");
        // One second later, the rate's worth more.
        now.set(Duration::from_secs(1));
        out.clear();
        for i in 0..100 {
            c.on_message(name(i), &mut out);
        }
        assert_eq!(c.live.as_ref().unwrap().page_events, EVENTS_BURST as usize + EVENTS_PER_SEC as usize);
        assert_eq!(warnings(&out).len(), 1);
        // Pings, audio and stop are not page events.
        now.set(Duration::from_secs(2));
        out.clear();
        c.on_message(Message::text(r#"{"type":"ping"}"#), &mut out);
        assert!(matches!(&out[..], [ServerMessage::Status(s)] if s.state == State::Recording));
        // The meeting's total.
        now.set(Duration::from_secs(3_600));
        c.live.as_mut().unwrap().page_events = MAX_PAGE_EVENTS - 1;
        out.clear();
        for i in 0..5 {
            c.on_message(name(i), &mut out);
        }
        assert_eq!(c.live.as_ref().unwrap().page_events, MAX_PAGE_EVENTS);
        assert_eq!(warnings(&out), ["too many page events for one meeting: ignoring the rest"]);
    }

    #[test]
    fn repeated_participant_lists_are_dropped_and_their_bytes_capped() {
        let tmp = tempfile::tempdir().unwrap();
        let host = Meetings(tmp.path().join("Sussurro"));
        let now = std::rc::Rc::new(std::cell::Cell::new(Duration::ZERO));
        let mut c = started(&host, now);
        let mut out = Vec::new();
        let list = |names: &str| Message::text(format!(r#"{{"type":"participants","names":[{names}]}}"#));
        c.on_message(list(r#""Anna","Bo""#), &mut out);
        // The same list (after cleaning) again and again: recorded once.
        for _ in 0..50 {
            c.on_message(list(r#""Anna","  Bo ""#), &mut out);
        }
        assert_eq!(c.live.as_ref().unwrap().page_events, 1);
        c.on_message(list(r#""Anna","Bo","Cy""#), &mut out);
        c.on_message(list(r#""Anna","Bo""#), &mut out);
        assert_eq!(c.live.as_ref().unwrap().page_events, 3, "a change back is new");
        assert!(warnings(&out).is_empty(), "{out:?}");
        // The meeting's byte budget.
        c.live.as_mut().unwrap().participants_bytes = MAX_PARTICIPANTS_BYTES - 5;
        c.on_message(list(r#""Dora","Eve""#), &mut out);
        assert_eq!(c.live.as_ref().unwrap().page_events, 3);
        assert_eq!(
            warnings(&out),
            ["too many participant updates for one meeting: ignoring the rest"]
        );
    }

    #[test]
    fn the_events_file_stays_open_while_recording_and_follows_a_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("Sussurro");
        let host = Meetings(archive.clone());
        let now = std::rc::Rc::new(std::cell::Cell::new(Duration::ZERO));
        let mut c = started(&host, now);
        let mut out = Vec::new();
        let id = meeting_item(&archive);
        let ev = |i: usize| Message::text(format!(r#"{{"type":"speaker_name","id":"csrc:{i}","name":"P{i}"}}"#));
        // Before the item exists the events wait; then they land in order.
        c.on_message(ev(0), &mut out);
        assert!(c.live.as_ref().unwrap().writer.is_none());
        {
            let live = c.live.as_mut().unwrap();
            live.item_id = Some(id.clone());
            live.flush_events();
        }
        for i in 1..4 {
            c.on_message(ev(i), &mut out);
        }
        let writer_id = c.live.as_ref().unwrap().writer.as_ref().map(|w| w.id().to_string());
        assert_eq!(writer_id.as_deref(), Some(id.as_str()), "one open file");
        assert_eq!(crate::archive::meeting::read_events(&archive, &id).unwrap().len(), 4);
        // Another id (a renamed folder): the writer follows it.
        let other = meeting_item(&archive);
        c.live.as_mut().unwrap().item_id = Some(other.clone());
        c.on_message(ev(4), &mut out);
        let writer_id = c.live.as_ref().unwrap().writer.as_ref().map(|w| w.id().to_string());
        assert_eq!(writer_id.as_deref(), Some(other.as_str()));
        assert_eq!(crate::archive::meeting::read_events(&archive, &other).unwrap().len(), 1);
        // An item that can't be written is not retried per event.
        c.live.as_mut().unwrap().item_id = Some("2026/09/missing".into());
        c.on_message(ev(5), &mut out);
        c.on_message(ev(6), &mut out);
        assert_eq!(c.live.as_ref().unwrap().unwritable.as_deref(), Some("2026/09/missing"));
        // The end of the audio closes the file.
        c.live.as_mut().unwrap().item_id = Some(id.clone());
        c.on_message(ev(7), &mut out);
        assert!(c.live.as_ref().unwrap().writer.is_some());
        c.end_audio();
        assert!(c.live.as_ref().unwrap().writer.is_none());
    }

    #[test]
    fn seq_jumps_insert_no_more_silence_than_the_wall_clock_allows() {
        use crate::sources::browser::SILENCE_GRACE_SAMPLES;
        let tmp = tempfile::tempdir().unwrap();
        let host = Meetings(tmp.path().join("Sussurro"));
        let now = std::rc::Rc::new(std::cell::Cell::new(Duration::ZERO));
        let mut c = started(&host, now.clone());
        let mut out = Vec::new();
        let pcm = [0i16; 320];
        // 5-byte frames (no samples) jumping a million frames each.
        c.on_message(Message::binary(protocol::encode_audio_frame(1, 0, &pcm)), &mut out);
        for k in 1..200u32 {
            c.on_message(Message::binary(protocol::encode_audio_frame(1, k * 1_000_000, &[])), &mut out);
        }
        let pos = c.live.as_ref().unwrap().mux.position();
        assert!(pos <= 320 + SILENCE_GRACE_SAMPLES, "{pos}");
        // With time passing, the budget grows with it, never faster.
        now.set(Duration::from_secs(10));
        c.on_message(Message::binary(protocol::encode_audio_frame(1, 4_000_000_000, &pcm)), &mut out);
        let pos = c.live.as_ref().unwrap().mux.position();
        assert!(pos <= 640 + SILENCE_GRACE_SAMPLES + 10 * 16_000, "{pos}");
    }

    #[test]
    fn with_no_meeting_waiting_for_events_ends_at_once() {
        let host = NoMeetings;
        let mut c = Conn::new(&host);
        c.end_audio();
        assert!(c.engine_messages(true).is_none());
    }
}
