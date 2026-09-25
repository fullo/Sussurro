/// <reference types="vite/client" />

/** Build target, replaced at build time (see vite.config.ts). */
declare const __BROWSER__: "chrome" | "firefox";

/** The few Chrome-only APIs the extension touches (the side panel, and the
 *  tab-capture fallback through an offscreen document); the rest goes
 *  through `webextension-polyfill`'s `browser`. */
declare const chrome:
  | {
      sidePanel?: {
        setPanelBehavior(behavior: { openPanelOnActionClick: boolean }): Promise<void>;
      };
      tabCapture?: {
        getMediaStreamId(options: { targetTabId: number }): Promise<string>;
      };
      offscreen?: {
        createDocument(options: { url: string; reasons: string[]; justification: string }): Promise<void>;
        closeDocument(): Promise<void>;
        hasDocument?(): Promise<boolean>;
      };
      runtime: {
        sendMessage(message: unknown): Promise<unknown>;
      };
      /** `storage.local.setAccessLevel` (Chrome ≥ 140 for `local`, #217). */
      storage?: {
        local?: import("./shared/secureStore").RestrictableArea;
      };
    }
  | undefined;
