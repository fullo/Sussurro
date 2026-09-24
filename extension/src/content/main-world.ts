/* MAIN-world content script (plan E4, #128): runs in the meeting page's own
 * JavaScript context at `document_start`, before the page's scripts, so it
 * can see every `RTCPeerConnection` and `getUserMedia` call.
 *
 * - **Always** (from page load): observe. Wrap the peer-connection API and
 *   `getUserMedia` and keep a registry of the audio tracks (registry.ts).
 *   Nothing is recorded, no audio graph exists, nothing leaves the page.
 * - **Only while armed** (the user pressed Start in the side panel; the
 *   ISOLATED script relays it): build one AudioContext with a mic bus and a
 *   remote bus, connect the selected tracks (following adds, replacements
 *   and removals), and stream PCM blocks from an AudioWorklet to the
 *   ISOLATED script over the private MessagePort from the handshake.
 *   Disarm tears the graph down.
 *
 * The tap never clones or stops the page's tracks, never mutes anything
 * and never calls `getUserMedia` — except the element fallback's own mic
 * request (see registry.ts), whose track it stops again on disarm. No
 * extension APIs here and no globals: this file shares the page's `window`
 * (the build wraps it in an IIFE). */
import { acceptPort } from "../shared/handshake";
import { FRAME_SAMPLES, rms16, toPcm16 } from "../shared/frame";
import type { CaptureSnapshot, FromMain, ToMain } from "../shared/messages";
import { TrackRegistry, diffTracks, type Selection } from "./registry";
import { PROCESSOR_NAME, WORKLET_SOURCE } from "./worklet";

type AnyFn = (...args: any[]) => any; // eslint-disable-line @typescript-eslint/no-explicit-any

(() => {
  const GUARD = Symbol.for("sussurro.capture.v1");
  const w = window as unknown as Record<symbol, unknown>;
  if (w[GUARD]) return; // injected twice (e.g. an extension reload)
  Object.defineProperty(w, GUARD, { value: true });

  // Natives, taken before the page can replace them.
  const NativePC = window.RTCPeerConnection;
  const NativeAudioContext = window.AudioContext;
  const NativeMediaStream = window.MediaStream;
  const NativeBlob = window.Blob;
  const createObjectURL = URL.createObjectURL.bind(URL);
  const revokeObjectURL = URL.revokeObjectURL.bind(URL);

  const reg = new TrackRegistry<MediaStreamTrack, RTCPeerConnection, RTCRtpSender>();
  const errors: string[] = [];
  const note = (msg: string) => {
    if (errors.length < 20 && !errors.includes(msg)) errors.push(msg);
  };
  let pcCount = 0;
  let port: MessagePort | null = null;
  let capture: Capture | null = null;

  const send = (m: FromMain, transfer: Transferable[] = []) => {
    try {
      port?.postMessage(m, transfer);
    } catch (e) {
      note(`post: ${String(e)}`);
    }
  };

  // ---- observation: RTCPeerConnection + getUserMedia -------------------------

  let pending = false;
  /** Something changed: re-select the tracks (once per microtask burst). */
  const changed = () => {
    if (!capture || pending) return;
    pending = true;
    queueMicrotask(() => {
      pending = false;
      capture?.apply();
    });
  };
  const watch = (t: MediaStreamTrack | null | undefined) => {
    if (t && t.kind === "audio") t.addEventListener("ended", changed);
  };

  /** Replace `obj[name]` with a same-named wrapper; hooks never break the call. */
  const wrap = (obj: object | undefined, name: string, before?: (self: any, args: any[]) => void, after?: (self: any, args: any[], result: any) => void) => {
    const o = obj as Record<string, unknown> | undefined;
    const orig = o?.[name];
    if (!o || typeof orig !== "function") return;
    const wrapped = {
      [name](this: unknown, ...args: unknown[]) {
        try {
          before?.(this, args);
        } catch (e) {
          note(`${name}: ${String(e)}`);
        }
        const result = (orig as AnyFn).apply(this, args);
        if (after) {
          const run = (r: unknown) => {
            try {
              after(this, args, r);
            } catch (e) {
              note(`${name}: ${String(e)}`);
            }
          };
          if (result && typeof (result as Promise<unknown>).then === "function") (result as Promise<unknown>).then(run, () => {});
          else run(result);
        }
        return result;
      },
    }[name];
    try {
      Object.defineProperty(o, name, { value: wrapped, writable: true, configurable: true, enumerable: false });
    } catch {
      o[name] = wrapped;
    }
  };

  const senderPc = new WeakMap<RTCRtpSender, RTCPeerConnection>();
  const scanSenders = (pc: RTCPeerConnection) => {
    for (const s of pc.getSenders()) {
      senderPc.set(s, pc);
      reg.setSender(pc, s, s.track);
      watch(s.track);
    }
  };
  const scanReceivers = (pc: RTCPeerConnection) => {
    for (const t of pc.getTransceivers()) {
      const dir = t.currentDirection;
      if ((dir === "sendrecv" || dir === "recvonly") && t.receiver.track) {
        reg.addRemote(pc, t.receiver.track);
        watch(t.receiver.track);
      }
    }
  };

  if (NativePC) {
    const proto = NativePC.prototype;
    wrap(proto, "addTrack", undefined, (pc: RTCPeerConnection, args, sender: RTCRtpSender) => {
      senderPc.set(sender, pc);
      reg.setSender(pc, sender, args[0]);
      watch(args[0]);
      changed();
    });
    wrap(proto, "addTransceiver", undefined, (pc: RTCPeerConnection, args, tr: RTCRtpTransceiver) => {
      senderPc.set(tr.sender, pc);
      if (args[0] && typeof args[0] === "object") {
        reg.setSender(pc, tr.sender, args[0]);
        watch(args[0]);
        changed();
      }
    });
    // Legacy stream API (still in some browsers): the senders tell.
    wrap(proto, "addStream", undefined, (pc: RTCPeerConnection) => {
      scanSenders(pc);
      changed();
    });
    wrap(proto, "removeTrack", (_pc, args) => {
      reg.removeSender(args[0]);
      changed();
    });
    wrap(proto, "setLocalDescription", undefined, (pc: RTCPeerConnection) => {
      scanSenders(pc);
      changed();
    });
    wrap(proto, "setRemoteDescription", undefined, (pc: RTCPeerConnection) => {
      scanSenders(pc);
      scanReceivers(pc);
      changed();
    });
    wrap(proto, "close", undefined, (pc: RTCPeerConnection) => {
      reg.closePc(pc);
      changed();
    });
    if (window.RTCRtpSender) {
      // Meet-like: the transceiver is negotiated first, the mic attached (or
      // switched to another device) later.
      wrap(RTCRtpSender.prototype, "replaceTrack", undefined, (sender: RTCRtpSender, args) => {
        const pc = senderPc.get(sender);
        if (pc) reg.setSender(pc, sender, args[0]);
        else reg.setSender(sender as unknown as RTCPeerConnection, sender, args[0]);
        watch(args[0]);
        changed();
      });
    }
    // The constructor: a Proxy keeps `prototype`, `instanceof` and statics.
    const Hooked = new Proxy(NativePC, {
      construct(target, args, newTarget) {
        const pc = Reflect.construct(target, args, newTarget) as RTCPeerConnection;
        pcCount++;
        reg.addPc(pc);
        pc.addEventListener("track", (e: RTCTrackEvent) => {
          reg.addRemote(pc, e.track);
          watch(e.track);
          changed();
        });
        changed();
        return pc;
      },
    });
    const g = window as unknown as Record<string, unknown>;
    g.RTCPeerConnection = Hooked;
    if (g.webkitRTCPeerConnection === NativePC) g.webkitRTCPeerConnection = Hooked;
  } else {
    note("no RTCPeerConnection in this page");
  }

  let ownGumCall = false;
  const md = window.MediaDevices?.prototype;
  const nativeGum = md?.getUserMedia;
  if (md && typeof nativeGum === "function") {
    wrap(md, "getUserMedia", undefined, (_self, _args, stream: MediaStream) => {
      if (ownGumCall || !stream) return;
      for (const t of stream.getAudioTracks()) {
        reg.addPageGum(t);
        watch(t);
      }
      changed();
    });
  }

  // ---- fallback: media elements (only when no peer connection exists) ------

  const captured = new WeakMap<HTMLMediaElement, MediaStream>();
  const elementTracks = (): MediaStreamTrack[] => {
    const out: MediaStreamTrack[] = [];
    for (const el of document.querySelectorAll<HTMLMediaElement>("audio, video")) {
      if (el.paused || el.ended) continue;
      const so = el.srcObject;
      if (so && so instanceof NativeMediaStream) {
        out.push(...so.getAudioTracks());
        continue;
      }
      // A URL source: mirror the element's output (does not steal its audio).
      let cs = captured.get(el);
      const cap = (el as HTMLMediaElement & { captureStream?: () => MediaStream }).captureStream;
      if (!cs && typeof cap === "function") {
        try {
          cs = cap.call(el);
          captured.set(el, cs);
        } catch (e) {
          note(`captureStream: ${String(e)}`);
        }
      }
      if (cs) out.push(...cs.getAudioTracks());
    }
    return out;
  };

  // ---- the audio graph (armed only) ------------------------------------------

  class Capture {
    readonly ctx: AudioContext;
    private merger: ChannelMergerNode;
    private buses: GainNode[];
    private sink: GainNode;
    private node: AudioWorkletNode | ScriptProcessorNode | null = null;
    private connected: Map<string, { track: MediaStreamTrack; src: MediaStreamAudioSourceNode }>[] = [new Map(), new Map()];
    private selection: Selection<MediaStreamTrack> | null = null;
    private ownTracks: MediaStreamTrack[] = [];
    private askedOwnMic = false;
    private seq = 0;
    private levels = [0, 0];
    private timer: ReturnType<typeof setInterval> | undefined;
    private resumeOnGesture = () => {
      void this.ctx.resume().catch(() => {});
    };
    processor: "audioworklet" | "scriptprocessor" | null = null;
    remoteOn: boolean;
    closed = false;

    constructor(remoteOn: boolean) {
      this.remoteOn = remoteOn;
      this.ctx = new NativeAudioContext();
      // Each bus downmixes its tracks to mono and sums them.
      this.buses = [0, 1].map(() => {
        const g = this.ctx.createGain();
        g.channelCount = 1;
        g.channelCountMode = "explicit";
        g.channelInterpretation = "speakers";
        return g;
      });
      this.merger = this.ctx.createChannelMerger(2);
      this.buses[0].connect(this.merger, 0, 0);
      this.buses[1].connect(this.merger, 0, 1);
      // Pulls the graph without making a sound.
      this.sink = this.ctx.createGain();
      this.sink.gain.value = 0;
      this.sink.connect(this.ctx.destination);
      // Autoplay policy: a context made without a recent gesture starts
      // suspended; the next click or key in the page resumes it.
      addEventListener("pointerdown", this.resumeOnGesture, true);
      addEventListener("keydown", this.resumeOnGesture, true);
    }

    async start(): Promise<void> {
      void this.ctx.resume().catch(() => {});
      const src = WORKLET_SOURCE;
      const urls: [string, () => string][] = [
        ["blob", () => createObjectURL(new NativeBlob([src], { type: "text/javascript" }))],
        ["data", () => "data:text/javascript;charset=utf-8," + encodeURIComponent(src)],
      ];
      for (const [kind, make] of urls) {
        let url = "";
        try {
          url = make();
          await this.ctx.audioWorklet.addModule(url);
          const node = new AudioWorkletNode(this.ctx, PROCESSOR_NAME, {
            numberOfInputs: 1,
            numberOfOutputs: 1,
            outputChannelCount: [1],
            channelCount: 2,
            channelCountMode: "explicit",
            channelInterpretation: "discrete",
            processorOptions: { block: FRAME_SAMPLES },
          });
          node.port.onmessage = (e: MessageEvent<{ mic: ArrayBuffer; remote: ArrayBuffer }>) =>
            this.block(new Int16Array(e.data.mic), new Int16Array(e.data.remote));
          this.node = node;
          this.processor = "audioworklet";
          break;
        } catch (e) {
          note(`worklet (${kind}): ${String(e)}`);
        } finally {
          if (kind === "blob" && url) revokeObjectURL(url);
        }
      }
      if (!this.node) {
        // Page CSP refused both module URLs: the deprecated main-thread node.
        const sp = this.ctx.createScriptProcessor(FRAME_SAMPLES, 2, 1);
        sp.channelCountMode = "explicit";
        sp.channelInterpretation = "discrete";
        sp.onaudioprocess = (e) => {
          const b = e.inputBuffer;
          const zero = new Float32Array(b.length);
          this.block(toPcm16(b.getChannelData(0)), toPcm16(b.numberOfChannels > 1 ? b.getChannelData(1) : zero));
        };
        this.node = sp;
        this.processor = "scriptprocessor";
      }
      if (this.closed) return;
      this.merger.connect(this.node).connect(this.sink);
      this.apply();
      this.timer = setInterval(() => this.tick(), 1000);
    }

    private block(mic: Int16Array, remote: Int16Array) {
      if (this.closed) return;
      this.levels = [rms16(mic), rms16(remote)];
      const m: FromMain = { t: "pcm", seq: this.seq++, mic: mic.buffer as ArrayBuffer, remote: this.remoteOn ? (remote.buffer as ArrayBuffer) : null };
      send(m, this.remoteOn ? [mic.buffer, remote.buffer] : [mic.buffer]);
    }

    /** Re-select the tracks and rewire the buses. */
    apply() {
      if (this.closed) return;
      reg.prune();
      if (!reg.pcSeen) reg.setElementTracks(elementTracks());
      const sel = reg.select();
      this.selection = sel;
      this.setChannel(0, sel.mic);
      this.setChannel(1, sel.remote);
      if (sel.wantOwnMic && !this.askedOwnMic) void this.ownMic();
    }

    private setChannel(ch: 0 | 1, tracks: MediaStreamTrack[]) {
      const map = this.connected[ch];
      const { add, remove } = diffTracks(
        [...map.values()].map((v) => v.track),
        tracks,
      );
      for (const t of remove) {
        const v = map.get(t.id);
        try {
          v?.src.disconnect();
        } catch {
          /* already gone */
        }
        map.delete(t.id);
      }
      for (const t of add) {
        try {
          // No clone(): when the page stops its track, capture stops too, so
          // the browser's mic indicator stays honest.
          const src = this.ctx.createMediaStreamSource(new NativeMediaStream([t]));
          src.connect(this.buses[ch]);
          map.set(t.id, { track: t, src });
        } catch (e) {
          note(`tap ${ch ? "remote" : "mic"}: ${String(e)}`);
        }
      }
    }

    /** Element fallback only: the page's mic was never seen, ask for one. */
    private async ownMic() {
      this.askedOwnMic = true;
      if (!nativeGum) return;
      ownGumCall = true;
      try {
        const s: MediaStream = await nativeGum.call(navigator.mediaDevices, { audio: true });
        if (this.closed) {
          s.getTracks().forEach((t) => t.stop());
          return;
        }
        for (const t of s.getAudioTracks()) {
          this.ownTracks.push(t);
          reg.addOwnGum(t);
        }
        changed();
      } catch (e) {
        note(`own getUserMedia: ${String(e)}`);
      } finally {
        ownGumCall = false;
      }
    }

    private tick() {
      // Watchdog: the ISOLATED script asks for state every couple of
      // seconds while armed. If it went away (extension reloaded or
      // removed), stop capturing instead of tapping the call for no one.
      if (Date.now() - lastCommandAt > WATCHDOG_MS) {
        note("extension went away: capture stopped");
        disarm();
        return;
      }
      this.apply();
      send({ t: "state", state: snapshot() });
    }

    snapshot(): Pick<CaptureSnapshot, "mic" | "remote"> {
      const sel = this.selection;
      return {
        mic: { via: sel?.micVia ?? "none", tracks: this.connected[0].size, level: this.levels[0] },
        remote: { via: sel?.remoteVia ?? "none", tracks: this.connected[1].size, level: this.levels[1] },
      };
    }

    close() {
      if (this.closed) return;
      this.closed = true;
      clearInterval(this.timer);
      removeEventListener("pointerdown", this.resumeOnGesture, true);
      removeEventListener("keydown", this.resumeOnGesture, true);
      for (const map of this.connected) {
        for (const v of map.values()) {
          try {
            v.src.disconnect();
          } catch {
            /* ignore */
          }
        }
        map.clear();
      }
      if (this.node instanceof AudioWorkletNode) this.node.port.postMessage("stop");
      // Our own mic request only; the page's tracks are never touched.
      for (const t of this.ownTracks) t.stop();
      void this.ctx.close().catch(() => {});
    }
  }

  const snapshot = (): CaptureSnapshot => {
    const parts = capture?.snapshot();
    return {
      armed: !!capture,
      ctxState: capture ? capture.ctx.state : null,
      processor: capture?.processor ?? null,
      pcCount,
      mic: parts?.mic ?? { via: reg.select().micVia, tracks: 0, level: 0 },
      remote: parts?.remote ?? { via: reg.select().remoteVia, tracks: 0, level: 0 },
      remoteExternal: capture ? !capture.remoteOn : false,
      errors: [...errors],
    };
  };

  // ---- commands from the ISOLATED script ------------------------------------

  /** Longest silence from the ISOLATED script while armed. */
  const WATCHDOG_MS = 10_000;
  let lastCommandAt = 0;
  const disarm = () => {
    capture?.close();
    capture = null;
    send({ t: "state", state: snapshot() });
  };

  const onCommand = async (m: ToMain) => {
    switch (m.t) {
      case "arm": {
        if (capture) {
          send({ t: "armed", rate: capture.ctx.sampleRate });
          return;
        }
        let c: Capture | null = null;
        try {
          c = new Capture(m.remote);
          capture = c;
          await c.start();
          if (capture !== c) return; // disarmed while starting
          send({ t: "armed", rate: c.ctx.sampleRate });
          send({ t: "state", state: snapshot() });
        } catch (e) {
          c?.close();
          if (capture === c) capture = null;
          send({ t: "arm-failed", error: String(e) });
        }
        return;
      }
      case "disarm":
        disarm();
        return;
      case "remote":
        if (capture) capture.remoteOn = m.on;
        return;
      case "state?":
        send({ t: "state", state: snapshot() });
        return;
    }
  };

  acceptPort(window, location.origin === "null" ? "*" : location.origin, (p) => {
    port = p;
    p.onmessage = (e: MessageEvent<ToMain>) => {
      if (!e.data || typeof e.data !== "object") return;
      lastCommandAt = Date.now();
      void onCommand(e.data);
    };
  });
})();
