/// <reference types="vite/client" />

/** Build target, replaced at build time (see vite.config.ts). */
declare const __BROWSER__: "chrome" | "firefox";

/** The one Chrome-only API the scaffold touches (the side panel); the rest
 *  goes through `webextension-polyfill`'s `browser`. */
declare const chrome:
  | {
      sidePanel?: {
        setPanelBehavior(behavior: { openPanelOnActionClick: boolean }): Promise<void>;
      };
    }
  | undefined;
