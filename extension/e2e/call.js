// Two-peer call for the capture harness (adapted from spike #104).
// A = "the user": the browser's fake microphone (440 Hz WAV in Chromium,
// the built-in 1 kHz source in Firefox). B = "the other participant": a
// 300 Hz oscillator. Each side has its own tone, so the fake app can prove
// the two channels are captured separately.
// `?add=` picks how A attaches its mic: track (addTrack), replace
// (addTransceiver('audio') then sender.replaceTrack — Meet-like).
const q = new URLSearchParams(location.search);
const role = q.get("role") || "A";
const add = role === "B" ? "track" : q.get("add") || "track";
const room = q.get("room") || "r";
const log = (...a) => {
  document.getElementById("log").textContent += a.join(" ") + "\n";
};
let pc, micTrack, pageCtx;
const sig = new WebSocket(`ws://${location.host}/signal?room=${encodeURIComponent(room)}`);
const send = (m) => sig.send(JSON.stringify(m));

async function localTrack() {
  if (role === "A") return (await navigator.mediaDevices.getUserMedia({ audio: true })).getAudioTracks()[0];
  pageCtx = pageCtx || new AudioContext();
  const osc = pageCtx.createOscillator();
  osc.frequency.value = 300;
  const g = pageCtx.createGain();
  g.gain.value = 0.3;
  const dst = pageCtx.createMediaStreamDestination();
  osc.connect(g).connect(dst);
  osc.start();
  return dst.stream.getAudioTracks()[0];
}

function playRemote(e) {
  const el = document.getElementById("remote");
  const stream = e.streams[0] || new MediaStream([e.track]);
  if (el.srcObject !== stream) {
    el.srcObject = stream;
    el.play().catch((err) => log("play() failed", err.name));
  }
}

async function join() {
  micTrack = await localTrack();
  pc = new window.__cachedPC();
  pc.ontrack = playRemote;
  pc.onicecandidate = (e) => e.candidate && send({ ice: e.candidate });
  pc.onconnectionstatechange = () => log("pc", pc.connectionState);
  if (add === "track") pc.addTrack(micTrack, new MediaStream([micTrack]));
  else pc.addTransceiver("audio", { direction: "sendrecv" });
  if (role === "A") {
    await pc.setLocalDescription(await pc.createOffer());
    send({ sdp: pc.localDescription });
  }
  log("joined as", role, "add=" + add);
}

sig.onmessage = async ({ data }) => {
  const m = JSON.parse(data);
  if (m.sdp) {
    await pc.setRemoteDescription(m.sdp);
    if (m.sdp.type === "offer") {
      await pc.setLocalDescription(await pc.createAnswer());
      send({ sdp: pc.localDescription });
    }
    if (add === "replace") {
      const t = pc.getTransceivers().find((t) => t.receiver.track.kind === "audio");
      await t.sender.replaceTrack(micTrack);
    }
  } else if (m.ice) await pc.addIceCandidate(m.ice).catch(() => {});
};

document.getElementById("join").onclick = join;

// Page-side truth, independent of the extension: audio flows both ways and
// the <audio> element keeps playing.
window.callState = async () => {
  const st = { role, connection: pc && pc.connectionState, inAudioLevel: 0 };
  if (pc)
    for (const r of (await pc.getStats()).values()) {
      if (r.type === "inbound-rtp" && r.kind === "audio") st.inAudioLevel = r.audioLevel || 0;
    }
  const el = document.getElementById("remote");
  st.audioEl = { paused: el.paused, muted: el.muted, volume: el.volume, currentTime: el.currentTime };
  st.micLive = !!micTrack && micTrack.readyState === "live";
  return st;
};
