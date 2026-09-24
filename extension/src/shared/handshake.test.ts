import { describe, expect, it } from "vitest";
import { HANDSHAKE_TAG, acceptPort, offerPort, type WindowLike } from "./handshake";

/** A window whose `postMessage` dispatches asynchronously, like a real one,
 *  to the listeners registered at dispatch time, in order. */
class FakeWindow implements WindowLike {
  listeners: ((e: MessageEvent) => void)[] = [];
  seen: unknown[] = [];
  postMessage(data: unknown, _origin: string, transfer: Transferable[] = []) {
    this.seen.push(data);
    setTimeout(() => {
      let stopped = false;
      const e = { data, source: this, ports: transfer, stopImmediatePropagation: () => (stopped = true) } as unknown as MessageEvent;
      for (const l of [...this.listeners]) {
        if (stopped) break;
        l(e);
      }
    }, 0);
  }
  addEventListener(_t: "message", l: (e: MessageEvent) => void) {
    this.listeners.push(l);
  }
  removeEventListener(_t: "message", l: (e: MessageEvent) => void) {
    this.listeners = this.listeners.filter((x) => x !== l);
  }
}

const tick = () => new Promise((r) => setTimeout(r, 5));

async function roundTrip(isolated: MessagePort, main: MessagePort) {
  const got = new Promise((r) => (main.onmessage = (e) => r(e.data)));
  isolated.postMessage({ t: "state?" });
  expect(await got).toEqual({ t: "state?" });
  isolated.close();
  main.close();
}

describe("MAIN ↔ ISOLATED handshake", () => {
  it("works when ISOLATED is injected first (Chrome's usual order)", async () => {
    const w = new FakeWindow();
    const isolated = offerPort(w, "*");
    await tick(); // the first offer finds nobody listening
    let main: MessagePort | null = null;
    acceptPort(w, "*", (p) => (main = p)); // main-ready → a new offer
    const port = await isolated;
    expect(main).not.toBeNull();
    await roundTrip(port, main!);
  });

  it("works when MAIN runs first and its main-ready is lost (Firefox)", async () => {
    const w = new FakeWindow();
    let main: MessagePort | null = null;
    acceptPort(w, "*", (p) => (main = p));
    await tick(); // main-ready dispatched to nobody
    const port = await offerPort(w, "*");
    await roundTrip(port, main!);
  });

  it("MAIN keeps the first port; later offers (page or duplicate) are ignored", async () => {
    const w = new FakeWindow();
    const ports: MessagePort[] = [];
    acceptPort(w, "*", (p) => ports.push(p));
    const port = await offerPort(w, "*");
    const extra = new MessageChannel();
    w.postMessage({ [HANDSHAKE_TAG]: "offer" }, "*", [extra.port2]);
    await tick();
    expect(ports).toHaveLength(1);
    await roundTrip(port, ports[0]);
    extra.port1.close();
  });

  it("ignores foreign messages and other windows", async () => {
    const w = new FakeWindow();
    const ports: MessagePort[] = [];
    acceptPort(w, "*", (p) => ports.push(p));
    const other = new MessageChannel();
    // Wrong source: an iframe's message.
    const foreign = { data: { [HANDSHAKE_TAG]: "offer" }, source: {}, ports: [other.port2], stopImmediatePropagation() {} } as unknown as MessageEvent;
    w.listeners.forEach((l) => l(foreign));
    w.postMessage({ hello: 1 }, "*");
    w.postMessage(null, "*");
    await tick();
    expect(ports).toHaveLength(0);
    other.port1.close();
  });

  it("hides the offer from listeners registered after MAIN (the page's)", async () => {
    const w = new FakeWindow();
    acceptPort(w, "*", (p) => p.close());
    const pageSaw: unknown[] = [];
    w.addEventListener("message", (e) => pageSaw.push(e.data));
    (await offerPort(w, "*")).close();
    await tick();
    expect(pageSaw.some((d) => (d as Record<string, unknown>)?.[HANDSHAKE_TAG] === "offer")).toBe(false);
  });
});
