/* The AudioWorklet that turns the two channel buses into PCM blocks.
 *
 * Its input has two channels (a ChannelMerger: 0 = mic bus, 1 = remote bus).
 * Every render quantum is appended to two i16 blocks of `block` samples;
 * a full pair is posted to the main thread (buffers transferred). A channel
 * with no track, or an inactive input, contributes silence, so both
 * channels always advance together and their `seq` stays gap-free — the app
 * then keeps them aligned without guessing. Post `"stop"` to end it.
 *
 * The source is a string (loaded from a Blob URL, else a data: URL): a
 * MAIN-world script has no extension URL of its own to load a module from.
 * `floatToPcm16` is inlined from its own source so the worklet and the unit
 * tests use the very same conversion. */
import { floatToPcm16 } from "../shared/frame";

export const PROCESSOR_NAME = "sussurro-capture";

export const WORKLET_SOURCE = `const cv = ${floatToPcm16.toString()};
class SussurroCapture extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.n = (options && options.processorOptions && options.processorOptions.block) || 2048;
    this.i = 0;
    this.fresh();
    this.on = true;
    this.port.onmessage = (e) => { if (e.data === "stop") this.on = false; };
  }
  fresh() { this.a = new Int16Array(this.n); this.b = new Int16Array(this.n); }
  process(inputs, outputs) {
    if (!this.on) return false;
    const input = inputs[0] || [];
    const mic = input[0], remote = input[1];
    const out = outputs[0] && outputs[0][0];
    const len = (mic && mic.length) || (remote && remote.length) || (out && out.length) || 128;
    for (let k = 0; k < len; k++) {
      this.a[this.i] = mic ? cv(mic[k]) : 0;
      this.b[this.i] = remote ? cv(remote[k]) : 0;
      if (++this.i === this.n) {
        this.port.postMessage({ mic: this.a.buffer, remote: this.b.buffer }, [this.a.buffer, this.b.buffer]);
        this.fresh();
        this.i = 0;
      }
    }
    return true;
  }
}
registerProcessor(${JSON.stringify(PROCESSOR_NAME)}, SussurroCapture);
`;
