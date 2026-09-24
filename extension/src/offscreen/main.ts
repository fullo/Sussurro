/* Chrome-only offscreen document: the last capture fallback (plan E4).
 *
 * When a meeting page shows no remote audio the hook can tap (no
 * peer-connection track, no playing media element — e.g. a client that
 * decodes audio in WebAssembly and plays it through WebAudio), the
 * background gets a `tabCapture` stream id for the tab and hands it here.
 * This document opens the tab's audio, plays it back (tab capture otherwise
 * silences the tab for the user) and streams it as the `remote` channel to
 * the background, through the same worklet as the page hook. The page's
 * mic still comes from the hook. Firefox has no `tabCapture`. */
import { FRAME_SAMPLES } from "../shared/frame";
import type { FromBackground, ToBackground, ToOffscreen } from "../shared/messages";
import { detectMode, encodePayload, type TransportMode } from "../shared/transport";
import { PROCESSOR_NAME, WORKLET_SOURCE } from "../content/worklet";

declare const chrome: {
  runtime: {
    connect(o: { name: string }): {
      postMessage(m: unknown): void;
      onMessage: { addListener(f: (m: unknown) => void): void };
      disconnect(): void;
    };
    onMessage: { addListener(f: (m: unknown) => void): void };
  };
};

let session: { ctx: AudioContext; stream: MediaStream; node: AudioWorkletNode } | null = null;
const port = chrome.runtime.connect({ name: "offscreen" });
let mode: TransportMode = "base64";
port.onMessage.addListener((raw) => {
  const m = raw as FromBackground;
  if (m.type === "hello") mode = detectMode(m.probe);
});

const post = (m: ToBackground) => port.postMessage(m);

async function start(streamId: string, rate: number) {
  stop();
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: { mandatory: { chromeMediaSource: "tab", chromeMediaSourceId: streamId } } as unknown as MediaTrackConstraints,
    video: false,
  });
  const ctx = new AudioContext({ sampleRate: rate });
  const src = ctx.createMediaStreamSource(stream);
  // Keep the meeting audible.
  src.connect(ctx.destination);
  const url = URL.createObjectURL(new Blob([WORKLET_SOURCE], { type: "text/javascript" }));
  try {
    await ctx.audioWorklet.addModule(url);
  } finally {
    URL.revokeObjectURL(url);
  }
  const bus = ctx.createGain();
  bus.channelCount = 1;
  bus.channelCountMode = "explicit";
  bus.channelInterpretation = "speakers";
  const merger = ctx.createChannelMerger(2);
  src.connect(bus).connect(merger, 0, 1); // input 0 (mic) stays silent
  const node = new AudioWorkletNode(ctx, PROCESSOR_NAME, {
    numberOfInputs: 1,
    numberOfOutputs: 1,
    outputChannelCount: [1],
    channelCount: 2,
    channelCountMode: "explicit",
    channelInterpretation: "discrete",
    processorOptions: { block: FRAME_SAMPLES },
  });
  let seq = 0;
  node.port.onmessage = (e: MessageEvent<{ remote: ArrayBuffer }>) => post({ type: "pcm", seq: seq++, remote: encodePayload(mode, e.data.remote) });
  const sink = ctx.createGain();
  sink.gain.value = 0;
  merger.connect(node).connect(sink).connect(ctx.destination);
  session = { ctx, stream, node };
}

function stop() {
  if (!session) return;
  session.node.port.postMessage("stop");
  session.stream.getTracks().forEach((t) => t.stop());
  void session.ctx.close().catch(() => {});
  session = null;
}

chrome.runtime.onMessage.addListener((raw) => {
  const m = raw as ToOffscreen;
  if (!m || (m as { target?: string }).target !== "offscreen") return;
  if (m.type === "offscreen:start") {
    start(m.streamId, m.rate).catch((e: unknown) => post({ type: "offscreen-error", error: String(e) }));
  } else if (m.type === "offscreen:stop") {
    stop();
  }
});
