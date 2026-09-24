/* What the side panel says and allows for a tab's state. Pure — unit
 * tested. (#128 ships the capture controls; the live transcript, speaker
 * chips and "Open in Sussurro" come with #129.) */
import type { PanelState } from "../shared/messages";
import { problemText } from "../shared/pairing";

export interface PanelView {
  /** One line about the connection / capture. */
  line: string;
  tone: "neutral" | "busy" | "live" | "problem";
  canStart: boolean;
  canStop: boolean;
}

const PLATFORM_NAME = { meet: "Google Meet", teams: "Microsoft Teams", zoom: "Zoom" } as const;

export function panelView(s: PanelState | null): PanelView {
  const view = (line: string, tone: PanelView["tone"], canStart = false, canStop = false): PanelView => ({ line, tone, canStart, canStop });
  if (!s) return view("Looking at this tab…", "busy");
  if (!s.paired) return view(problemText("not-paired"), "problem");
  if (!s.meetingPage && s.phase === "idle") {
    return view("Open a Google Meet, Microsoft Teams or Zoom web call in this tab (reload it if it was already open).", "neutral");
  }
  const where = s.platform ? PLATFORM_NAME[s.platform] : "this page";
  switch (s.phase) {
    case "checking":
      return view("Checking the Sussurro app…", "busy", false, true);
    case "arming":
      return view(`Starting capture on ${where}…`, "busy", false, true);
    case "connecting":
      return view("Connecting to the Sussurro app…", "busy", false, true);
    case "live":
      return view(`Recording ${where}. Sussurro transcribes it as you go.`, "live", false, true);
    case "reconnecting": {
      const why = s.problem === "not-running" ? " The app is not reachable." : "";
      return view(`Connection lost, retrying (attempt ${s.attempt}).${why} Audio is kept for a few seconds.`, "problem", false, true);
    }
    case "stopping":
      return view("Stopping: Sussurro is finishing the transcript…", "busy");
    case "done":
      return view("Saved in Sussurro.", "neutral", true);
    case "error":
      return view(s.problem ? problemText(s.problem) : (s.message ?? "Capture failed."), "problem", true);
    case "idle":
    default:
      if (s.app && !s.app.ok) return view(problemText(s.app.problem), "problem", s.app.problem === "not-running");
      return view(`Ready to record ${where}. Nothing is captured until you press Start.`, "neutral", true);
  }
}

/** "peer-connection" → words for the channel's source. */
export function viaText(via: string): string {
  switch (via) {
    case "peer-connection":
      return "call audio";
    case "media-element":
      return "page audio (fallback)";
    case "sender":
      return "your microphone";
    case "page-getUserMedia":
      return "your microphone (page)";
    case "tab audio":
      return "tab audio (Chrome fallback)";
    case "own-getUserMedia":
      return "your microphone (extra permission)";
    default:
      return "not found yet";
  }
}
