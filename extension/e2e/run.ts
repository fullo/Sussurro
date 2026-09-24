/* Capture harness (#128): the real extension build in real browsers, a local
 * two-peer WebRTC call and a fake Sussurro app. Proves, per browser, that
 * after Start in the side panel both channels reach `/live` separately
 * (each side of the call has its own tone), `seq` is gap-free, the call
 * keeps working both ways, Stop ends the meeting, and closing the tab mid-
 * capture sends `stop`.
 *
 *   npm run build && npm run test:e2e            (all configurations)
 *   npm run test:e2e -- chromium firefox          (a subset)
 *   HEADED=1 npm run test:e2e -- chromium
 *
 * Configurations: `chromium` (structured-clone messaging, Meet-like
 * `replaceTrack`), `chromium-json` (the manifest key removed: base64 over
 * JSON messaging, as on Chrome < 148), `firefox` (a temporary add-on
 * installed over the remote debugging protocol — Playwright can't load
 * Firefox extensions itself; the harness drives its background over RDP).
 *
 * Needs the Playwright browsers (`npx playwright install chromium firefox`;
 * set PLAYWRIGHT_BROWSERS_PATH to keep them out of the home folder).
 * Temporary files go to $E2E_TMPDIR (default: the OS temp dir). */
import { chromium, firefox, type BrowserContext, type Page } from "playwright";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Rdp } from "./rdp.ts";
import { rms, startServer, toneShare, type LiveSession } from "./server.ts";

const EXT = fileURLToPath(new URL("..", import.meta.url));
const TOKEN = "e2e0".repeat(16);
const GECKO_ID = "sussurro@darumahq.it";
const FIREFOX_UUID = "5b7f3a52-6c1e-4f0e-9c1a-2d8b3c4e5f60";
const headless = !process.env.HEADED;
const CAPTURE_MS = 6000;

type Config = "chromium" | "chromium-json" | "firefox";
const ALL: Config[] = ["chromium", "chromium-json", "firefox"];
const wanted = process.argv.slice(2).filter((a) => !a.startsWith("-")) as Config[];
const configs = wanted.length ? wanted : ALL;

const tmpRoot = mkdtempSync(join(process.env.E2E_TMPDIR ?? tmpdir(), "sussurro-e2e-"));
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until<T>(what: string, f: () => T | Promise<T>, ms = 10_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = await f();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(100);
  }
}

/** The built extension, copied, with the content scripts also matching the
 *  local call page (the shipped manifest only matches the meeting sites). */
function testExtension(target: "chrome" | "firefox", config: Config): string {
  const dir = join(tmpRoot, `ext-${config}`);
  cpSync(join(EXT, "dist", target), dir, { recursive: true });
  const mf = JSON.parse(readFileSync(join(dir, "manifest.json"), "utf8"));
  for (const cs of mf.content_scripts) cs.matches.push("http://127.0.0.1/*");
  if (config === "chromium-json") delete mf.message_serialization;
  writeFileSync(join(dir, "manifest.json"), JSON.stringify(mf, null, 2));
  return dir;
}

/** 3 s of a 440 Hz tone, 48 kHz mono 16-bit: Chromium's fake microphone. */
function toneWav(): string {
  const rate = 48000;
  const n = rate * 3;
  const b = Buffer.alloc(44 + n * 2);
  b.write("RIFF", 0);
  b.writeUInt32LE(36 + n * 2, 4);
  b.write("WAVEfmt ", 8);
  b.writeUInt32LE(16, 16);
  b.writeUInt16LE(1, 20);
  b.writeUInt16LE(1, 22);
  b.writeUInt32LE(rate, 24);
  b.writeUInt32LE(rate * 2, 28);
  b.writeUInt16LE(2, 32);
  b.writeUInt16LE(16, 34);
  b.write("data", 36);
  b.writeUInt32LE(n * 2, 40);
  for (let i = 0; i < n; i++) b.writeInt16LE(Math.round(Math.sin((2 * Math.PI * 440 * i) / rate) * 0.5 * 32767), 44 + 2 * i);
  const file = join(tmpRoot, "tone440.wav");
  writeFileSync(file, b);
  return file;
}

/** The bits of the side panel the harness uses. */
interface Panel {
  click(testId: string): Promise<void>;
  /** `data-*` attribute of `[data-testid=status]`. */
  status(attr: "phase" | "transport"): Promise<string | null>;
  canClick(testId: string): Promise<boolean>;
  text(): Promise<string>;
}

function pagePanel(p: Page): Panel {
  return {
    click: (id) => p.click(`[data-testid="${id}"]`),
    status: (attr) => p.locator('[data-testid="status"]').getAttribute(`data-${attr}`),
    canClick: (id) => p.locator(`[data-testid="${id}"]:not([disabled])`).isVisible(),
    text: () => p.locator("main").innerText(),
  };
}

interface Launched {
  ctx: BrowserContext;
  /** Store the pairing, as the options page does (#127's keys). */
  pair(port: number, token: string): Promise<void>;
  /** The tab id of the first tab whose URL contains `part`. */
  tabIdOf(part: string): Promise<number>;
  /** The side panel, opened as a tab controlling `tabId`. */
  openPanel(tabId: number): Promise<Panel>;
  micTone: number;
  close(): Promise<void>;
}

async function launchChromium(config: Config): Promise<Launched> {
  const ext = testExtension("chrome", config);
  const ctx = await chromium.launchPersistentContext(join(tmpRoot, `profile-${config}`), {
    channel: "chromium",
    headless,
    permissions: ["microphone"],
    args: [
      `--disable-extensions-except=${ext}`,
      `--load-extension=${ext}`,
      "--use-fake-device-for-media-stream",
      `--use-file-for-fake-audio-capture=${toneWav()}`,
      "--autoplay-policy=no-user-gesture-required",
    ],
  });
  const sw = ctx.serviceWorkers()[0] ?? (await ctx.waitForEvent("serviceworker", { timeout: 15_000 }));
  const id = new URL(sw.url()).host;
  return {
    ctx,
    micTone: 440,
    async pair(port, token) {
      await sw.evaluate(([port, token]) => (globalThis as any).chrome.storage.local.set({ port, token }), [port, token] as const);
    },
    tabIdOf: (part) =>
      sw.evaluate(async (part) => {
        const tabs = await (globalThis as any).chrome.tabs.query({});
        return tabs.find((t: any) => (t.url ?? "").includes(part)).id as number;
      }, part),
    async openPanel(tabId) {
      const p = await ctx.newPage();
      await p.goto(`chrome-extension://${id}/sidepanel.html?tabId=${tabId}`);
      return pagePanel(p);
    },
    close: () => ctx.close(),
  };
}

async function freePort(): Promise<number> {
  const srv = net.createServer();
  await new Promise<void>((r) => srv.listen(0, "127.0.0.1", r));
  const port = (srv.address() as net.AddressInfo).port;
  await new Promise((r) => srv.close(r));
  return port;
}

async function launchFirefox(config: Config): Promise<Launched> {
  const ext = testExtension("firefox", config);
  const rdp = await freePort();
  const home = join(tmpRoot, "ff-home");
  mkdirSync(home, { recursive: true });
  const ctx = await firefox.launchPersistentContext(join(tmpRoot, `profile-${config}`), {
    headless,
    args: ["-start-debugger-server", String(rdp)],
    // macOS: Firefox looks for ~/Library/Application Support/Firefox and
    // exits when it can't read it; keep it inside the temp dir.
    env: { ...process.env, CFFIXED_USER_HOME: home } as Record<string, string>,
    firefoxUserPrefs: {
      "devtools.debugger.remote-enabled": true,
      "devtools.chrome.enabled": true,
      "devtools.debugger.prompt-connection": false,
      "media.navigator.streams.fake": true,
      "media.navigator.permission.disabled": true,
      "media.autoplay.default": 0,
      "media.autoplay.block-webaudio": false,
      // A fixed internal UUID, so the harness knows the moz-extension:// URL.
      "extensions.webextensions.uuids": JSON.stringify({ [GECKO_ID]: FIREFOX_UUID }),
    },
  });
  const rdpc = await Rdp.connect(rdp);
  const id = await rdpc.installTemporaryAddon(ext);
  const targets = await rdpc.watchAddon(id);
  const target = (part: string): Promise<string> => until(`the ${part} document`, () => [...targets].find(([url]) => url.includes(part))?.[1] ?? "");
  // Looked up per call: Firefox may suspend and restart the event page.
  const bg = async (code: string) => rdpc.evaluate(await target("background"), code);
  return {
    ctx,
    micTone: 1000,
    async pair(port, token) {
      await bg(`browser.storage.local.set(${JSON.stringify({ port, token })}); 0`);
      await until("the pairing to be stored", async () => (await bg("browser.storage.local.get('token').then(r => globalThis.__e2eToken = r.token); globalThis.__e2eToken")) === token);
    },
    async tabIdOf(part) {
      const code = `browser.tabs.query({}).then(ts => globalThis.__e2eTab = (ts.find(t => (t.url || "").includes(${JSON.stringify(part)})) || {}).id); globalThis.__e2eTab`;
      return (await until("the tab id", async () => (await bg(code)) as number)) as number;
    },
    async openPanel(tabId) {
      // Playwright doesn't see moz-extension:// tabs: script the panel over RDP.
      await bg(`browser.tabs.create({ url: browser.runtime.getURL("sidepanel.html?tabId=${tabId}") }); 0`);
      const c = await target("sidepanel.html");
      const js = (code: string) => rdpc.evaluate(c, code);
      const q = (id: string) => `document.querySelector('[data-testid="${id}"]')`;
      return {
        async click(id) {
          await until(`${id} to be clickable`, async () => (await js(`!!${q(id)} && !${q(id)}.disabled`)) === true);
          await js(`${q(id)}.click(); 0`);
        },
        status: async (attr) => ((await js(`${q("status")}?.getAttribute("data-${attr}") ?? null`)) as string | null) ?? null,
        canClick: async (id) => (await js(`!!${q(id)} && !${q(id)}.disabled`)) === true,
        text: async () => String(await js(`document.querySelector("main")?.innerText ?? ""`)),
      };
    },
    async close() {
      rdpc.close();
      await ctx.close();
    },
  };
}

// ---- one configuration ------------------------------------------------------------

interface Check {
  name: string;
  ok: boolean;
  detail?: string;
}

async function runConfig(config: Config): Promise<Check[]> {
  const checks: Check[] = [];
  const check = (name: string, ok: boolean, detail?: unknown) => {
    checks.push({ name, ok, detail: detail === undefined ? undefined : typeof detail === "string" ? detail : JSON.stringify(detail) });
  };
  const server = await startServer(TOKEN);
  const b = config === "firefox" ? await launchFirefox(config) : await launchChromium(config);
  let shownPanel: Panel | null = null;
  try {
    await b.pair(server.port, TOKEN);

    const room = `${config}-${Date.now()}`;
    const base = `http://127.0.0.1:${server.port}/call.html?room=${room}&add=${config === "chromium" ? "replace" : "track"}`;
    const B = await b.ctx.newPage();
    await B.goto(`${base}&role=B`);
    await B.click("#join");
    const A = await b.ctx.newPage();
    await A.goto(`${base}&role=A`);
    await A.click("#join");
    await until("the call to connect", () => A.evaluate(() => (window as any).callState().then((s: any) => s.connection === "connected")), 15_000);

    check("the MAIN-world hook is installed in the call page", await A.evaluate(() => Symbol.for("sussurro.capture.v1") in window));
    const panel = await b.openPanel(await b.tabIdOf("role=A"));
    shownPanel = panel;
    const phase = () => panel.status("phase");

    // 1. Nothing before Start.
    await until("Start to be enabled", () => panel.canClick("start"));
    check("no socket before Start", server.sessions.length === 0);

    // 2. Start → live, both channels.
    await panel.click("start");
    await until("phase live", async () => (await phase()) === "live", 15_000);
    const transport = await panel.status("transport");
    check(`transport is ${config === "chromium-json" ? "base64" : "binary"}`, transport === (config === "chromium-json" ? "base64" : "binary"), transport);
    await sleep(CAPTURE_MS);
    const a = await A.evaluate(() => (window as any).callState());
    const bs = await B.evaluate(() => (window as any).callState());
    await panel.click("stop");
    await until("phase done", async () => (await phase()) === "done", 10_000);

    const s: LiveSession | undefined = server.sessions[0];
    check("one /live session, from the extension origin", server.sessions.length === 1 && /^(chrome|moz)-extension:\/\//.test(s?.origin ?? ""), s?.origin);
    const start = s?.start ?? {};
    const rate = Number(start.rate);
    check(
      "start {title, url, platform, rate, channels}",
      start.title === "e2e call" && String(start.url).includes("role=A") && start.platform === "other" && rate >= 8000 && rate <= 192000 && start.channels === 2,
      start,
    );
    const mic = s?.channels.get(0);
    const remote = s?.channels.get(1);
    const minFrames = Math.floor(((CAPTURE_MS / 1000) * rate) / 2048 / 2);
    const describe = (c: typeof mic) =>
      c && { frames: c.frames, firstSeq: c.firstSeq, gaps: c.seqGaps, rms: +rms(c).toFixed(4), mic: +toneShare(c.tail, rate, b.micTone).toFixed(3), remote: +toneShare(c.tail, rate, 300).toFixed(3) };
    check("mic channel (0) arrives", !!mic && mic.frames >= minFrames && rms(mic) > 0.01, describe(mic));
    check("remote channel (1) arrives", !!remote && remote.frames >= minFrames && rms(remote) > 0.01, describe(remote));
    check(
      "channels are separate (own tone on each)",
      !!mic && !!remote && toneShare(mic.tail, rate, b.micTone) > 0.8 && toneShare(mic.tail, rate, 300) < 0.05 && toneShare(remote.tail, rate, 300) > 0.8 && toneShare(remote.tail, rate, b.micTone) < 0.05,
    );
    check("seq starts at 0 with no gaps", !!mic && !!remote && mic.firstSeq === 0 && remote.firstSeq === 0 && mic.seqGaps === 0 && remote.seqGaps === 0);
    check("channels stay aligned", !!mic && !!remote && Math.abs(mic.frames - remote.frames) <= 2, [mic?.frames, remote?.frames]);
    check("no malformed frames", (s?.badFrames ?? 1) === 0);
    check("stop sent, socket closed", !!s?.stopped && s.closed);
    check("call unaffected: others hear you", bs.inAudioLevel > 0.001, bs.inAudioLevel);
    check("call unaffected: you hear them", a.inAudioLevel > 0.001 && !a.audioEl.paused && !a.audioEl.muted && a.audioEl.volume === 1, a);
    check("the page's mic track is untouched", a.micLive === true);

    // 3. Teardown: closing the tab mid-capture ends the meeting.
    await panel.click("start");
    await until("second session live", () => server.sessions[1]?.start && (server.sessions[1].channels.get(0)?.frames ?? 0) > 5, 15_000);
    await A.close();
    const second = await until("stop after the tab closed", () => (server.sessions[1].stopped ? server.sessions[1] : null), 10_000).catch(() => null);
    check("closing the tab sends stop", !!second);
    await B.close();
  } catch (e) {
    const shown = shownPanel ? await shownPanel.text().catch(() => "") : "";
    check("harness ran", false, `${String(e instanceof Error ? e.stack : e)}\n    side panel: ${shown.replace(/\s+/g, " ")}`);
  } finally {
    await b.close().catch(() => {});
    await server.close();
  }
  return checks;
}

// ---- main ------------------------------------------------------------------------------

let failed = 0;
const t0 = Date.now();
for (const config of configs) {
  const t = Date.now();
  const checks = await runConfig(config);
  console.log(`\n${config} (${((Date.now() - t) / 1000).toFixed(1)} s)`);
  for (const c of checks) {
    console.log(`  ${c.ok ? "ok  " : "FAIL"} ${c.name}${c.detail && (!c.ok || process.env.VERBOSE) ? `  ${c.detail}` : ""}`);
    if (!c.ok) failed++;
  }
}
try {
  // macOS: Firefox's stand-in home gets "deny delete" ACLs on Library & co.
  if (process.platform === "darwin") execFileSync("chmod", ["-R", "-N", tmpRoot]);
  rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
} catch (e) {
  console.warn(`could not remove ${tmpRoot}: ${String(e)}`);
}
console.log(`\n${failed ? `${failed} check(s) failed` : "all checks passed"} in ${((Date.now() - t0) / 1000).toFixed(1)} s`);
process.exit(failed ? 1 : 0);
