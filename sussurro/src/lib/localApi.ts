/* Settings → Browser extension (#127): what to tell the user about the local
   API, which the extension talks to. Its settings apply at startup, so the
   running state (`local_api_status`) can lag behind them. Pure. */
import type { ListenState } from "./types";

export interface ApiNotice {
  /** "ok": the extension can reach Sussurro on the configured port. */
  tone: "ok" | "warn";
  text: string;
  /** Offer to switch the local API on. */
  offerEnable: boolean;
}

export function apiNotice(apiEnabled: boolean, port: number, status: ListenState | null): ApiNotice {
  const running = status?.state === "listening" ? status.port : null;
  if (!apiEnabled) {
    return {
      tone: "warn",
      text:
        running !== null
          ? "The local API is switched off: it stops when Sussurro restarts, and the extension can't reach Sussurro without it."
          : "The local API is off. The extension reaches Sussurro through it: switch it on, then restart Sussurro.",
      offerEnable: true,
    };
  }
  if (running === port) {
    return { tone: "ok", text: `Local API listening on 127.0.0.1:${port}.`, offerEnable: false };
  }
  if (running !== null) {
    return {
      tone: "warn",
      text: `The local API listens on port ${running}; port ${port} applies when Sussurro restarts. Copy the pairing code again after the restart.`,
      offerEnable: false,
    };
  }
  if (status?.state === "failed") {
    return {
      tone: "warn",
      text:
        status.port === port
          ? `The local API could not listen on port ${port}: another program may be using it. Choose another port in Behavior → Advanced, then restart Sussurro.`
          : `The local API could not listen on port ${status.port}. Restart Sussurro to try port ${port}.`,
      offerEnable: false,
    };
  }
  return { tone: "warn", text: "The local API starts when Sussurro restarts.", offerEnable: false };
}
