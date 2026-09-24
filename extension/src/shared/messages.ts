/* Messages between the extension's parts (types only).
 *
 *   MAIN world ──MessagePort──▶ ISOLATED world ──runtime port "capture"──▶ background ──WebSocket──▶ app
 *   (hook, AudioWorklet)        (relay)                                  (per-tab session)
 *                                              offscreen (Chrome tab capture) ──port "offscreen"──▶ background
 *   side panel ◀──runtime messages──▶ background
 */
import type { MicVia, RemoteVia } from "../content/registry";
import type { Platform } from "./platform";
import type { AppProblem } from "./appcheck";
import type { TransportMode } from "./transport";
import type { SubtitlesMode } from "./connection";
import type { LiveAction, LiveTranscript } from "./live";
import type { Phase } from "../background/session";

/** What the page-side capture reports (about once a second while armed,
 *  and on request). */
export interface CaptureSnapshot {
  armed: boolean;
  /** `AudioContext.state` (`suspended` until the page had a user gesture). */
  ctxState: string | null;
  /** `audioworklet` or the `scriptprocessor` fallback. */
  processor: string | null;
  /** Peer connections the page created since load. */
  pcCount: number;
  mic: { via: MicVia; tracks: number; level: number };
  remote: { via: RemoteVia; tracks: number; level: number };
  /** The remote channel is left to Chrome's tab capture. */
  remoteExternal: boolean;
  errors: string[];
}

// ---- MAIN ↔ ISOLATED (MessagePort) -------------------------------------------

export type ToMain = { t: "arm"; remote: boolean } | { t: "disarm" } | { t: "remote"; on: boolean } | { t: "state?" };

export type FromMain =
  | { t: "armed"; rate: number }
  | { t: "arm-failed"; error: string }
  | { t: "pcm"; seq: number; mic: ArrayBuffer; remote: ArrayBuffer | null }
  | { t: "state"; state: CaptureSnapshot };

// ---- content / offscreen ↔ background (runtime ports) -----------------------

/** A payload is an ArrayBuffer on a binary-safe port, else base64. */
export type Payload = ArrayBuffer | string;

export type ToBackground =
  | { type: "armed"; rate: number; title: string; url: string; platform: Platform | null; transport: TransportMode }
  | { type: "arm-failed"; error: string }
  | { type: "pcm"; seq: number; mic?: Payload; remote?: Payload }
  | { type: "state"; state: CaptureSnapshot }
  | { type: "offscreen-error"; error: string };

export type FromBackground =
  | { type: "hello"; probe: ArrayBuffer }
  | { type: "arm"; remote: boolean }
  | { type: "disarm" }
  | { type: "remote"; on: boolean };

/** `tabs.sendMessage` to a meeting page's ISOLATED script. */
export type ToPage = { type: "page:info" } | { type: "page:connect" };

export interface PageInfo {
  platform: Platform | null;
  title: string;
  url: string;
  state: CaptureSnapshot | null;
}

// ---- side panel ↔ background -------------------------------------------------

export interface PanelState {
  tabId: number;
  /** The tab runs our content script (a supported meeting page). */
  meetingPage: boolean;
  platform: Platform | null;
  paired: boolean;
  /** Result of the last app check (null: not checked yet). */
  app: { ok: true; version: string; subtitles?: SubtitlesMode } | { ok: false; problem: AppProblem } | null;
  phase: Phase;
  problem?: AppProblem;
  message?: string;
  itemId?: string;
  serverState?: string;
  attempt: number;
  capture: CaptureSnapshot | null;
  tabCapture: "off" | "starting" | "on" | "failed";
  /** How audio crosses the page → background port (diagnostics). */
  transport: TransportMode | null;
}

export type PanelRequest =
  | { type: "panel:get"; tabId: number }
  /** The tab's live transcript (#129): a snapshot, then `panel:live`. */
  | { type: "panel:transcript"; tabId: number }
  | { type: "panel:start"; tabId: number }
  | { type: "panel:stop"; tabId: number };

/** Broadcast by the background to every open panel. */
export type PanelBroadcast =
  | { type: "panel:state"; state: PanelState }
  /** One change to a tab's live transcript (#129): an app → extension
   *  `/live` message, or a reset on a new Start. `rev` is the transcript's
   *  revision after it; `epoch` names that transcript (a restarted
   *  background starts a new one). See `live.ts` (`mirrorReceive`). */
  | { type: "panel:live"; tabId: number; epoch: string; rev: number; action: LiveAction };

export type { LiveTranscript };

// ---- background ↔ offscreen document (Chrome) -------------------------------

export type ToOffscreen =
  | { type: "offscreen:start"; target: "offscreen"; streamId: string; rate: number }
  | { type: "offscreen:stop"; target: "offscreen" };
