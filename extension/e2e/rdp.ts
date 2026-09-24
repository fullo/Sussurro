/* A minimal Firefox remote debugging protocol client for the harness:
 * install a temporary add-on and evaluate code in its background page.
 * (Playwright can neither load a Firefox extension nor open its
 * moz-extension:// pages.) Packets are `<length>:<json>` over TCP. */
import net from "node:net";

type Packet = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

export class Rdp {
  private sock: net.Socket;
  private buf = Buffer.alloc(0);
  private waiters: { match: (p: Packet) => boolean; resolve: (p: Packet) => void }[] = [];
  private listeners: ((p: Packet) => void)[] = [];
  private backlog: Packet[] = [];

  private constructor(sock: net.Socket) {
    this.sock = sock;
    sock.on("data", (d) => this.onData(d));
  }

  /** Connect and wait for the root actor's greeting (retrying while Firefox starts). */
  static async connect(port: number, attempts = 50): Promise<Rdp> {
    let last: unknown;
    for (let i = 0; i < attempts; i++) {
      try {
        const sock = await new Promise<net.Socket>((resolve, reject) => {
          const s = net.connect(port, "127.0.0.1", () => resolve(s));
          s.once("error", reject);
        });
        const rdp = new Rdp(sock);
        await rdp.next((p) => p.from === "root", 5000);
        return rdp;
      } catch (e) {
        last = e;
        await new Promise((r) => setTimeout(r, 200));
      }
    }
    throw last;
  }

  private onData(d: Buffer) {
    this.buf = Buffer.concat([this.buf, d]);
    for (;;) {
      const i = this.buf.indexOf(58);
      if (i < 0) return;
      const len = Number(this.buf.subarray(0, i).toString());
      if (this.buf.length < i + 1 + len) return;
      const p = JSON.parse(this.buf.subarray(i + 1, i + 1 + len).toString()) as Packet;
      this.buf = this.buf.subarray(i + 1 + len);
      if (process.env.RDP_DEBUG) console.log("rdp<", JSON.stringify(p).slice(0, 200));
      for (const l of this.listeners) l(p);
      const w = this.waiters.findIndex((x) => x.match(p));
      if (w >= 0) this.waiters.splice(w, 1)[0].resolve(p);
      else {
        // Kept for a waiter registered a moment later (two packets can
        // arrive in one chunk, before the first one's awaiter resumes).
        this.backlog.push(p);
        if (this.backlog.length > 500) this.backlog.shift();
      }
    }
  }

  next(match: (p: Packet) => boolean, timeoutMs = 10_000): Promise<Packet> {
    const early = this.backlog.findIndex(match);
    if (early >= 0) return Promise.resolve(this.backlog.splice(early, 1)[0]);
    return new Promise((resolve, reject) => {
      const waiter = { match, resolve };
      this.waiters.push(waiter);
      setTimeout(() => {
        const i = this.waiters.indexOf(waiter);
        if (i >= 0) {
          this.waiters.splice(i, 1);
          reject(new Error("RDP: no answer"));
        }
      }, timeoutMs);
    });
  }

  /** Send a request and wait for the actor's reply (a packet without `type`). */
  request(to: string, type: string, extra: Packet = {}): Promise<Packet> {
    const reply = this.next((p) => p.from === to && (p.type === undefined || p.error !== undefined));
    const s = JSON.stringify({ to, type, ...extra });
    if (process.env.RDP_DEBUG) console.log("rdp>", s.slice(0, 200));
    this.sock.write(`${Buffer.byteLength(s)}:${s}`);
    return reply.then((p) => {
      if (p.error) throw new Error(`RDP ${type}: ${p.error} ${p.message ?? ""}`);
      return p;
    });
  }

  async installTemporaryAddon(path: string): Promise<string> {
    const root = await this.request("root", "getRoot");
    const r = await this.request(root.addonsActor, "installTemporaryAddon", { addonPath: path });
    return r.addon.id;
  }

  /** Watch an add-on's documents (background page, extension tabs):
   *  returns their console actors by URL, kept up to date. */
  async watchAddon(id: string): Promise<Map<string, string>> {
    const { addons } = await this.request("root", "listAddons");
    const addon = (addons as Packet[]).find((a) => a.id === id);
    if (!addon) throw new Error(`RDP: add-on ${id} not found`);
    // Current Firefox: descriptor → watcher → one target per document.
    const { actor: watcher } = await this.request(addon.actor, "getWatcher", { isServerTargetSwitchingEnabled: true });
    const targets = new Map<string, string>();
    this.listeners.push((p) => {
      if (p.from !== watcher) return;
      if (p.type === "target-available-form") targets.set(String(p.target.url), p.target.consoleActor);
      if (p.type === "target-destroyed-form") targets.delete(String(p.target?.url));
    });
    await this.request(watcher, "watchTargets", { targetType: "frame" });
    return targets;
  }

  /** Evaluate `text` in a document of the add-on; returns the result
   *  (primitives as themselves, objects as grips). */
  async evaluate(consoleActor: string, text: string): Promise<unknown> {
    const { resultID } = await this.request(consoleActor, "evaluateJSAsync", { text });
    const r = await this.next((p) => p.from === consoleActor && p.type === "evaluationResult" && p.resultID === resultID);
    if (r.exceptionMessage) throw new Error(`RDP evaluate: ${r.exceptionMessage}`);
    const g = r.result;
    // Grips for the values primitives can't carry in JSON.
    if (g && typeof g === "object") {
      if (g.type === "undefined") return undefined;
      if (g.type === "null") return null;
      if (g.type === "longString") return g.initial;
    }
    return g;
  }

  close() {
    this.sock.end();
  }
}
