/* The side panel's item buttons (#129): "Open in Sussurro", "Copy as text",
 * "Create .srt". Which are shown and usable is pure (unit tested); the
 * clipboard and download helpers below touch the DOM. */
import type { SubtitlesMode } from "../shared/connection";
import type { Phase } from "../background/session";

export type ItemAction = "open" | "copy" | "srt";

export interface ActionState {
  visible: boolean;
  enabled: boolean;
  /** Why it is disabled (the button's title). */
  hint?: string;
}

export interface ActionsInput {
  /** The meeting's item in the app (null: none yet). */
  itemId: string | null;
  lines: number;
  phase: Phase | null;
  /** The app's subtitles setting (unknown: an app that doesn't say). */
  subtitles?: SubtitlesMode;
  /** The action running now. */
  busy: ItemAction | null;
}

/** Still recording, or the app is finishing the transcript. */
export function isRecording(phase: Phase | null): boolean {
  return phase === "checking" || phase === "arming" || phase === "connecting" || phase === "live" || phase === "reconnecting" || phase === "stopping";
}

export function actionsView(i: ActionsInput): Record<ItemAction, ActionState> {
  const hasItem = i.itemId !== null;
  const idle = i.busy === null;
  const noLines = i.lines === 0 ? "Nothing transcribed yet." : undefined;
  const srtBlocked = isRecording(i.phase) ? "Available when the recording ends." : noLines;
  return {
    open: { visible: hasItem, enabled: hasItem && idle },
    copy: { visible: hasItem, enabled: hasItem && idle && !noLines, hint: noLines },
    // With "always", the app writes transcript.srt itself.
    srt: { visible: hasItem && i.subtitles === "on_request", enabled: hasItem && idle && !srtBlocked, hint: srtBlocked },
  };
}

/** Put text on the clipboard: the async API, else the legacy copy command
 *  (the async one can refuse once the click's activation expired). */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    try {
      return document.execCommand("copy");
    } catch {
      return false;
    } finally {
      area.remove();
    }
  }
}

/** Save text as a file through an anchor on a Blob URL: no `downloads`
 *  permission needed. */
export function downloadText(text: string, filename: string, type: string): void {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.rel = "noopener";
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}
