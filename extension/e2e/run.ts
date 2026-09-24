/* Capture harness (#128): the real extension build in real browsers, a local
 * two-peer WebRTC call and a fake Sussurro app. Proves, per browser, that
 * after Start in the side panel both channels reach `/live` separately
 * (each side of the call has its own tone), `seq` is gap-free, the call
 * keeps working both ways, Stop ends the meeting, and closing the tab mid-
 * capture sends `stop`. The side panel (#129) shows the fake app's live
 * lines with their speaker chips and backlog, and its Open in Sussurro /
 * Copy as text / Create .srt reach the app's item routes. On a fake Meet
 * page, the Meet name observer (#131) sends the contributing-
 * source timeline, the bound names, the participants and its health.
 *
 *   npm run build && npm run test:e2e            (chromium, chromium-json, firefox)
 *   npm run test:e2e -- chromium firefox edge     (a choice)
 *   HEADED=1 npm run test:e2e -- chromium
 *
 * Configurations: `chromium` (structured-clone messaging, Meet-like
 * `replaceTrack`), `chromium-json` (the manifest key removed: base64 over
 * JSON messaging, as on Chrome < 148), `firefox` (a temporary add-on
 * installed over the remote debugging protocol — Playwright can't load
 * Firefox extensions itself; the harness drives its background over RDP,
 * with a short event-page idle timeout, and also opens the real sidebar).
 * Only when named: `edge` (the installed Microsoft Edge, Playwright's
 * `msedge` channel) and `brave` (the installed Brave, or BRAVE_PATH), both
 * with the Chrome build.
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
import { LIVE_SCRIPT, rms, startServer, toneShare, type LiveSession } from "./server.ts";

const EXT = fileURLToPath(new URL("..", import.meta.url));
const TOKEN = "e2e0".repeat(16);
const GECKO_ID = "sussurro@darumahq.it";
const FIREFOX_UUID = "5b7f3a52-6c1e-4f0e-9c1a-2d8b3c4e5f60";
/** Firefox's id for the add-on's sidebar (its widget id + a suffix). */
const SIDEBAR_ID = "sussurro_darumahq_it-sidebar-action";
const SIDEBAR_URL = `moz-extension://${FIREFOX_UUID}/sidepanel.html`;
const headless = !process.env.HEADED;
const CAPTURE_MS = 6000;
/** A tab id no tab has. */
const NO_TAB = 999_999;
/** Firefox's event-page idle timeout in the harness (default 30 s): short,
 *  so that everything the extension does while capturing, and while the app
 *  finishes after Stop (FINISH_MS), outlasts it several times over. */
const FIREFOX_IDLE_MS = 2000;
const FINISH_MS = 5000;

type Config = "chromium" | "chromium-json" | "firefox" | "edge" | "brave";
/** Run by default (and in CI). `edge` and `brave` (the Chrome build in the
 *  installed browsers) run only when named. */
const ALL: Config[] = ["chromium", "chromium-json", "firefox"];
const EXTRA: Config[] = ["edge", "brave"];
const wanted = process.argv.slice(2).filter((a) => !a.startsWith("-")) as Config[];
const unknown = wanted.filter((c) => !ALL.includes(c) && !EXTRA.includes(c));
if (unknown.length) throw new Error(`unknown configuration(s): ${unknown.join(", ")} (${[...ALL, ...EXTRA].join(", ")})`);
const configs = wanted.length ? wanted : ALL;

/** Where Brave lives when BRAVE_PATH is not set. */
const BRAVE_DEFAULT: Partial<Record<NodeJS.Platform, string>> = {
  darwin: "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
  linux: "/usr/bin/brave-browser",
  win32: "C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe",
};

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
  /** The inner text of every element matching `selector`. */
  texts(selector: string): Promise<string[]>;
}

function pagePanel(p: Page): Panel {
  return {
    click: (id) => p.click(`[data-testid="${id}"]`),
    status: (attr) => p.locator('[data-testid="status"]').getAttribute(`data-${attr}`),
    canClick: (id) => p.locator(`[data-testid="${id}"]:not([disabled])`).isVisible(),
    text: () => p.locator("main").innerText(),
    texts: (selector) => p.locator(selector).allInnerTexts(),
  };
}

interface Sidebar extends Panel {
  hide(): Promise<void>;
  show(): Promise<void>;
  /** Make a new blank tab the window's active one (true), or go back to
   *  the first tab and close it (false). */
  otherTab(on: boolean): Promise<void>;
}

interface Launched {
  ctx: BrowserContext;
  /** Store the pairing, as the options page does (#127's keys). */
  pair(port: number, token: string): Promise<void>;
  /** The tab id of the first tab whose URL contains `part`. */
  tabIdOf(part: string): Promise<number>;
  /** The side panel, opened as a tab controlling `tabId`. */
  openPanel(tabId: number): Promise<Panel>;
  /** Firefox: the real sidebar, in the window of the first tab whose URL
   *  contains `part`. */
  openSidebar?(part: string): Promise<Sidebar>;
  micTone: number;
  /** Firefox: how many times the event page was suspended so far (it
   *  runs with a short idle timeout, see FIREFOX_IDLE_MS). */
  suspends?(): Promise<number>;
  close(): Promise<void>;
}

async function launchChromium(config: Config): Promise<Launched> {
  const ext = testExtension("chrome", config);
  const ctx = await chromium.launchPersistentContext(join(tmpRoot, `profile-${config}`), {
    // Playwright's Chromium; Edge through its `msedge` channel (the
    // installed Edge); Brave by path. Branded builds ignore --load-extension
    // unless this feature is switched off.
    ...(config === "edge" ? { channel: "msedge" } : config === "brave" ? { executablePath: process.env.BRAVE_PATH ?? BRAVE_DEFAULT[process.platform] } : { channel: "chromium" }),
    headless,
    // "Create .srt" downloads a file: keep it in the temp dir.
    downloadsPath: join(tmpRoot, "downloads"),
    permissions: ["microphone"],
    args: [
      `--disable-extensions-except=${ext}`,
      `--load-extension=${ext}`,
      "--use-fake-device-for-media-stream",
      `--use-file-for-fake-audio-capture=${toneWav()}`,
      "--autoplay-policy=no-user-gesture-required",
      ...(config === "edge" || config === "brave" ? ["--disable-features=DisableLoadExtensionCommandLineSwitch"] : []),
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
    downloadsPath: join(tmpRoot, "downloads"),
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
      "extensions.background.idle.timeout": FIREFOX_IDLE_MS,
      // A fixed internal UUID, so the harness knows the moz-extension:// URL.
      "extensions.webextensions.uuids": JSON.stringify({ [GECKO_ID]: FIREFOX_UUID }),
    },
  });
  const rdpc = await Rdp.connect(rdp);
  // The parent process (chrome privileges), for two things no extension
  // document can do. Firefox never suspends an event page while DevTools
  // are attached to its add-on, and the harness is attached (RDP): make
  // Firefox ignore that for this add-on, so its idle timeout applies as it
  // does for users. And count the suspensions.
  const parent = await rdpc.parentProcessConsole();
  const chromeJs = (code: string) => rdpc.evaluate(parent, code);
  await chromeJs(`(() => {
    const { ExtensionParent } = ChromeUtils.importESModule("resource://gre/modules/ExtensionParent.sys.mjs");
    const du = ExtensionParent.DebugUtils;
    const attached = du.hasDevToolsAttached.bind(du);
    du.hasDevToolsAttached = (id) => id !== ${JSON.stringify(GECKO_ID)} && attached(id);
    return 0;
  })()`);
  const id = await rdpc.installTemporaryAddon(ext);
  const extension = `WebExtensionPolicy.getByID(${JSON.stringify(id)}).extension`;
  await chromeJs(`(() => {
    globalThis.__e2eSuspends = 0;
    ${extension}.on("background-script-suspend", () => { globalThis.__e2eSuspends++; });
    return 0;
  })()`);
  const targets = await rdpc.watchAddon(id);
  const target = (part: string): Promise<string> => until(`the ${part} document`, () => [...targets].find(([url]) => url.includes(part))?.[1] ?? "");
  /** A panel document scripted over RDP (found per call: the sidebar's
   *  document goes away when it is closed). */
  const rdpPanel = (urlEnd: string): Panel => {
    const js = async (code: string) => rdpc.evaluate(await until(`the ${urlEnd} document`, () => [...targets].find(([url]) => url.endsWith(urlEnd))?.[1] ?? ""), code);
    const q = (id: string) => `document.querySelector('[data-testid="${id}"]')`;
    return {
      async click(id) {
        await until(`${id} to be clickable`, async () => (await js(`!!${q(id)} && !${q(id)}.disabled`)) === true);
        await js(`${q(id)}.click(); 0`);
      },
      status: async (attr) => ((await js(`${q("status")}?.getAttribute("data-${attr}") ?? null`)) as string | null) ?? null,
      canClick: async (id) => (await js(`!!${q(id)} && !${q(id)}.disabled`)) === true,
      text: async () => String(await js(`document.querySelector("main")?.innerText ?? ""`)),
      texts: async (selector) => JSON.parse(String(await js(`JSON.stringify([...document.querySelectorAll(${JSON.stringify(selector)})].map((e) => e.innerText))`))) as string[],
    };
  };
  // The event page may be suspended (idle) or just restarting: wake it,
  // then evaluate in its current document.
  const bg = async (code: string): Promise<unknown> => {
    for (let attempt = 0; ; attempt++) {
      await chromeJs(`${extension}.wakeupBackground(); 0`);
      try {
        return await rdpc.evaluate(await target("_generated_background_page"), code);
      } catch (e) {
        if (attempt >= 20) throw e;
        await sleep(200);
      }
    }
  };
  // The value of a promise-valued expression: RDP's evaluation returns at
  // once, so the result is parked under a key of its own and polled.
  let parked = 0;
  const bgAwait = async (expr: string): Promise<unknown> => {
    const key = `__e2e${++parked}`;
    await bg(`Promise.resolve().then(() => ${expr}).then((v) => { globalThis.${key} = JSON.stringify({ v: v ?? null }); }, (e) => { globalThis.${key} = JSON.stringify({ e: String(e) }); }); 0`);
    const r = JSON.parse(String(await until(`the background to answer ${expr}`, () => bg(`globalThis.${key} ?? ""`)))) as { v?: unknown; e?: string };
    if (r.e !== undefined) throw new Error(`background: ${r.e}`);
    return r.v;
  };
  return {
    ctx,
    micTone: 1000,
    suspends: async () => Number(await chromeJs("globalThis.__e2eSuspends")),
    async pair(port, token) {
      await bg(`browser.storage.local.set(${JSON.stringify({ port, token })}); 0`);
      await until("the pairing to be stored", async () => (await bgAwait("browser.storage.local.get('token').then((r) => r.token)")) === token);
    },
    async tabIdOf(part) {
      const code = `browser.tabs.query({}).then((ts) => (ts.find((t) => (t.url || "").includes(${JSON.stringify(part)})) || {}).id)`;
      return (await until("the tab id", async () => (await bgAwait(code)) as number)) as number;
    },
    async openPanel(tabId) {
      // Playwright doesn't see moz-extension:// tabs: script the panel over RDP.
      await bg(`browser.tabs.create({ url: browser.runtime.getURL("sidepanel.html?tabId=${tabId}") }); 0`);
      return rdpPanel(`sidepanel.html?tabId=${tabId}`);
    },
    async openSidebar(part) {
      // The real sidebar, as the toolbar button opens it (sidebarAction
      // .open() needs a user action, so through the browser window), in
      // the window of that tab. It has no ?tabId=: it follows the window's
      // active tab, made the page's tab here (Playwright gives each page a
      // window of its own, where openPanel's tab may be the active one).
      await chromeJs(`(() => {
        const has = (t) => t.linkedBrowser.currentURI.spec.includes(${JSON.stringify(part)});
        const w = [...Services.wm.getEnumerator("navigator:browser")].find((w) => w.gBrowser.tabs.some(has));
        globalThis.__e2eSidebarWindow = w;
        globalThis.__e2eSidebarTab = w.gBrowser.tabs.find(has);
        w.gBrowser.selectedTab = globalThis.__e2eSidebarTab;
        w.SidebarController.show(${JSON.stringify(SIDEBAR_ID)});
        return 0;
      })()`);
      const sidebar = rdpPanel(SIDEBAR_URL);
      const w = "globalThis.__e2eSidebarWindow";
      return {
        ...sidebar,
        async hide() {
          await chromeJs(`${w}.SidebarController.hide(); 0`);
          await until("the sidebar to close", () => ![...targets.keys()].some((url) => url.endsWith(SIDEBAR_URL)));
        },
        show: async () => void (await chromeJs(`${w}.SidebarController.show(${JSON.stringify(SIDEBAR_ID)}); 0`)),
        async otherTab(on) {
          await chromeJs(
            on
              ? `(() => { const g = ${w}.gBrowser; g.selectedTab = g.addTab("about:blank", { triggeringPrincipal: Services.scriptSecurityManager.getSystemPrincipal() }); return 0; })()`
              : `(() => { const g = ${w}.gBrowser; const extra = g.selectedTab; g.selectedTab = globalThis.__e2eSidebarTab; g.removeTab(extra); return 0; })()`,
          );
        },
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
  const server = await startServer(TOKEN, { finishMs: FINISH_MS });
  const b = config === "firefox" ? await launchFirefox(config) : await launchChromium(config);
  let shownPanel: Panel | null = null;
  try {
    // Not paired yet: the panel says so, and follows the pairing once it
    // is stored (storage.local change events).
    const unpaired = await b.openPanel(NO_TAB);
    const unpairedText = await until("the not-paired note", async () => ((await unpaired.text()).includes("Not paired") ? await unpaired.text() : ""), 10_000).catch(() => "");
    check("before pairing, the panel says it is not paired", unpairedText.includes("Not paired with the Sussurro app"), unpairedText);
    await b.pair(server.port, TOKEN);
    const paired = await until("the panel to see the pairing", async () => {
      const text = await unpaired.text();
      return !text.includes("Not paired") && text.includes("Open a Google Meet") ? text : "";
    }).catch(async () => unpaired.text());
    check("once paired, the panel drops the note and follows the tab", paired.includes("Open a Google Meet"), paired);

    // 0. Firefox's event page (#137): with nothing going on it is suspended
    //    after the idle timeout — so the lifetime checks below mean something.
    if (b.suspends) {
      const before = await b.suspends();
      await sleep(FIREFOX_IDLE_MS * 2 + 1000);
      check("Firefox: the event page is suspended when idle", (await b.suspends()) > before);
    }

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

    // 1b. The first Start shows the recording notice (#136) and starts
    //     nothing until it is answered; Cancel starts nothing at all.
    await panel.click("start");
    await until("the recording notice", () => panel.canClick("notice-proceed"));
    await panel.click("notice-cancel");
    await until("Start back after Cancel", () => panel.canClick("start"));
    check("the first Start shows the notice; Cancel starts nothing", server.sessions.length === 0);

    // 2. Start → notice → Start recording → live, both channels.
    await panel.click("start");
    await until("the recording notice again", () => panel.canClick("notice-proceed"));
    const suspendsAtStart = await b.suspends?.();
    await panel.click("notice-proceed");
    await until("phase live", async () => (await phase()) === "live", 15_000);
    const reminder = (await panel.texts("[data-testid=reminder]"))[0] ?? "";
    check("the reminder line shows while recording", reminder.includes("Recording other people"), reminder);
    const transport = await panel.status("transport");
    check(`transport is ${config === "chromium-json" ? "base64" : "binary"}`, transport === (config === "chromium-json" ? "base64" : "binary"), transport);
    const t0 = Date.now();

    // 2b. The side panel mirrors the fake app's live transcript (#129).
    const lastLine = (LIVE_SCRIPT[4] as { segment: { text: string } }).segment.text;
    await until("the live lines in the side panel", async () => (await panel.texts("[data-testid=transcript] .tx-text")).some((x) => x.includes(lastLine)), 10_000);
    const lines = (await panel.texts("[data-testid=transcript] .tx-text")).map((x) => x.trim());
    check(
      "live lines render, the correction replacing the first line",
      JSON.stringify(lines) === JSON.stringify(["Hello from the far side, corrected.", "Hi Anna, loud and clear.", "A third voice joins."]),
      lines,
    );
    const chips = (await panel.texts("[data-testid=transcript] .tx-chip")).map((x) => x.trim());
    check("speaker chips: the app's name, You on the mic, Voice N", JSON.stringify(chips) === JSON.stringify(["Anna", "You", "Voice 2"]), chips);
    const times = (await panel.texts("[data-testid=transcript] .tx-time")).map((x) => x.trim());
    check("lines carry their timestamps", JSON.stringify(times) === JSON.stringify(["00:00:01", "00:00:02", "00:00:03"]), times);
    const backlog = (await panel.texts("[data-testid=backlog]"))[0] ?? "";
    check("the backlog indicator shows the app's status", backlog.includes("7 s behind"), backlog);
    check("Create .srt waits for the end of the recording", !(await panel.canClick("action-srt")) && (await panel.texts("[data-testid=action-srt]")).length === 1);
    await panel.click("action-open");
    const opened = await until("Open in Sussurro", () => server.items.find((r) => r.method === "POST" && r.path === "/items/e2e-1/open"), 5000).catch(() => null);
    check("Open in Sussurro → POST /items/{id}/open", !!opened, server.items);
    await panel.click("action-copy");
    const copied = await until("Copy as text", () => server.items.find((r) => r.path === "/items/e2e-1/export" && r.format === "txt"), 5000).catch(() => null);
    check("Copy as text → GET /items/{id}/export?format=txt", !!copied, server.items);

    // 2c. Firefox: the real sidebar (#137), opened mid-meeting in the call
    //     tab's window, shows the meeting so far and follows the window's
    //     active tab; Stop is pressed there.
    let stopFrom: Panel = panel;
    if (b.openSidebar) {
      const sidebar = await b.openSidebar("role=A");
      const lines = () => sidebar.texts("[data-testid=transcript] .tx-text");
      const restored = await until("the lines in the sidebar", async () => ((await lines()).length === 3 ? await lines() : null), 10_000).catch(() => null);
      check("sidebar: opened mid-meeting, shows the lines so far", JSON.stringify(restored) === JSON.stringify(await panel.texts("[data-testid=transcript] .tx-text")), restored);
      check("sidebar: live, with the reminder", (await sidebar.status("phase")) === "live" && (await sidebar.texts("[data-testid=reminder]")).length === 1);
      await sidebar.otherTab(true);
      const elsewhere = await until("the sidebar to follow the active tab", async () => ((await sidebar.text()).includes("Open a Google Meet") ? await sidebar.text() : ""), 5000).catch(() => "");
      check("sidebar: follows the window's active tab", elsewhere !== "" && (await lines()).length === 0, elsewhere);
      await sidebar.otherTab(false);
      await until("the sidebar back on the call", async () => (await sidebar.status("phase")) === "live" && (await lines()).length === 3, 5000).catch(() => null);
      await sidebar.hide();
      await sidebar.show();
      const reopened = await until("the reopened sidebar", async () => ((await lines()).length === 3 ? await lines() : null), 10_000).catch(() => null);
      check("sidebar: closed and reopened, the lines are back", !!reopened, reopened);
      stopFrom = sidebar;
    }

    await sleep(Math.max(0, CAPTURE_MS - (Date.now() - t0)));
    const a = await A.evaluate(() => (window as any).callState());
    const bs = await B.evaluate(() => (window as any).callState());
    await stopFrom.click("stop");
    // The app works through its backlog for FINISH_MS before `done`.
    const finishing = await until("phase stopping", async () => (await phase()) === "stopping", 5000).catch(() => false);
    check("Stop → stopping while the app finishes", !!finishing);
    await until("phase done", async () => (await phase()) === "done", FINISH_MS + 10_000);
    if (b.suspends) {
      const n = (await b.suspends()) - (suspendsAtStart ?? 0);
      check(`Firefox: the event page stays up from Start until the app is done (${FIREFOX_IDLE_MS / 1000} s idle timeout)`, n === 0, `${n} suspension(s)`);
    }
    await panel.click("action-srt");
    const srt = await until("Create .srt", () => server.items.find((r) => r.path === "/items/e2e-1/export" && r.format === "srt"), 5000).catch(() => null);
    const note = await until("the .srt note", async () => (await panel.texts("[data-testid=action-note]")).find((x) => x.includes(".srt")) ?? "", 5000).catch(() => "");
    check("after Stop, Create .srt downloads GET /items/{id}/export?format=srt", !!srt && note.includes("e2e-1.srt"), note || server.items);
    check("the lines stay after Stop", (await panel.texts("[data-testid=transcript] .tx-text")).length === 3);

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

    // 3. Teardown: closing the tab mid-capture ends the meeting. "Don't
    //    show this again" was ticked (the default): no notice this time.
    await panel.click("start");
    check("the notice is not shown again once acknowledged", (await panel.texts("[data-testid=notice]")).length === 0);
    await until("second session live", () => server.sessions[1]?.start && (server.sessions[1].channels.get(0)?.frames ?? 0) > 5, 15_000);
    // A new Start is a new meeting: the panel shows only its lines (the
    // script again), not the first meeting's plus a "connection lost" part.
    const again = await panel.texts("[data-testid=transcript] .tx-text");
    check("a new Start clears the previous meeting's lines", again.length === 3 && !(await panel.text()).includes("Connection lost"), again);
    await A.close();
    const second = await until("stop after the tab closed", () => (server.sessions[1].stopped ? server.sessions[1] : null), 10_000).catch(() => null);
    check("closing the tab sends stop", !!second);
    await B.close();

    // 4. Meet names (#131): a fake Meet page (served at meet.google.com by
    //    a route, so the shipped content-script patterns match it) with
    //    tiles and faked contributing sources, in every browser.
    await meetNames();

    // 5. Firefox: the keep-alive lets go once no meeting is on.
    if (b.suspends) {
      const before = await b.suspends();
      await sleep(FIREFOX_IDLE_MS * 2 + 1000);
      check("Firefox: the event page is suspended again after the meetings", (await b.suspends()) > before);
    }
  } catch (e) {
    const shown = shownPanel ? await shownPanel.text().catch(() => "") : "";
    check("harness ran", false, `${String(e instanceof Error ? e.stack : e)}\n    side panel: ${shown.replace(/\s+/g, " ")}`);
  } finally {
    await b.close().catch(() => {});
    await server.close();
  }
  return checks;

  async function meetNames() {
    await b.ctx.route("https://meet.google.com/**", (r) =>
      r.fulfill({ status: 200, contentType: "text/html; charset=utf-8", body: readFileSync(join(EXT, "e2e", "meet.html"), "utf8") }),
    );
    const M = await b.ctx.newPage();
    await M.goto("https://meet.google.com/e2e-fake");
    await M.click("#join");
    await until("the fake Meet call", () => M.evaluate(() => (window as any).callState().joined), 10_000);
    const meetPanel = await b.openPanel(await b.tabIdOf("meet.google.com"));
    shownPanel = meetPanel;
    await until("Start on the Meet tab", () => meetPanel.canClick("start"));
    const before = server.sessions.length;
    await meetPanel.click("start");
    await until("Meet session live", async () => (await meetPanel.status("phase")) === "live", 15_000);
    await sleep(9_000);
    await meetPanel.click("stop");
    await until("Meet session done", async () => (await meetPanel.status("phase")) === "done", 10_000);
    const ms = server.sessions[before];
    const ctl = (type: string) => (ms?.controls ?? []).filter((c) => c.type === type);
    check("Meet: start says platform meet", ms?.start?.platform === "meet", ms?.start);
    const act = ctl("speaker_active");
    check(
      "Meet: speaker_active from the contributing sources (rtp, csrc ids, t on the audio clock)",
      act.some((c) => c.id === "csrc:1001" && c.source === "rtp") &&
        act.some((c) => c.id === "csrc:1002") &&
        act.every((c, i) => typeof c.t === "number" && c.t >= 0 && c.t <= 15_000 && (i === 0 || (c.t as number) >= (act[i - 1].t as number) - 500)),
      act,
    );
    check("Meet: speaker_idle when a speaker stops", ctl("speaker_idle").some((c) => c.id === "csrc:1001"), ctl("speaker_idle"));
    const bound = Object.fromEntries(ctl("speaker_name").map((c) => [c.id, c.name]));
    check("Meet: names bound to the sources by the lit tiles", bound["csrc:1001"] === "Bo E2e" && bound["csrc:1002"] === "Cy E2e", bound);
    const people = ctl("participants").flatMap((c) => c.names as string[]);
    check("Meet: participants without the user", people.includes("Bo E2e") && people.includes("Cy E2e") && !people.includes("Ada E2e"), people);
    const health = ctl("observer_health").at(-1);
    check("Meet: observer health ok, with the selector set", health?.state === "ok" && health?.set === "meet-2026-09a", health);
    await M.close();
  }
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
