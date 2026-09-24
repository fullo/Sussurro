/* Link source (#123): pure helpers for New → Link. The rules themselves
   (schemes, local hosts, platforms) live in Rust (`sources/url`) and reach
   the UI through `link_inspect`; these only word and measure. */

import type { Run } from "./engineRuns";
import type { EngineDownload, LinkInfo, LinkKind, LinkVia, YtDlpStatus } from "./types";

/** Worth asking `link_inspect` about: something that looks like a link. */
export function looksLikeLink(input: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(input.trim());
}

/** "Direct media" / "Video platform (yt-dlp)". */
export function kindLabel(kind: LinkKind): string {
  return kind === "platform" ? "Video platform (yt-dlp)" : "Direct media";
}

/** 1536 → "1.5 KB", 3_500_000 → "3.3 MB" (binary units, one decimal). */
export function formatBytes(n: number): string {
  if (!(n >= 0) || !Number.isFinite(n)) return "";
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${Math.round(v)} B` : `${v.toFixed(1)} ${units[i]}`;
}

/** Percent downloaded (0–100), or null when the total is unknown. */
export function downloadPercent(d: EngineDownload | null): number | null {
  if (!d || !d.total_bytes || d.total_bytes <= 0) return null;
  return Math.max(0, Math.min(100, Math.round((d.downloaded_bytes / d.total_bytes) * 100)));
}

/** "12.3 MB of 45.6 MB · 27%" / "12.3 MB" / "Connecting…". */
export function describeDownload(d: EngineDownload | null): string {
  if (!d || d.downloaded_bytes === 0) return d?.via === "yt-dlp" ? "Asking yt-dlp for the audio track…" : "Connecting…";
  const pct = downloadPercent(d);
  const got = formatBytes(d.downloaded_bytes);
  return pct === null ? got : `${got} of ${formatBytes(d.total_bytes ?? 0)} · ${pct}%`;
}

export function viaLabel(via: LinkVia): string {
  return via === "yt-dlp" ? "yt-dlp" : "direct download";
}

/** The two phases a link run shows: download, then transcription (once the
 *  engine has opened the item). */
export type LinkPhase = "download" | "transcribe";

export function linkPhase(run: Pick<Run, "itemId" | "progress">): LinkPhase {
  return run.itemId !== null || run.progress !== null ? "transcribe" : "download";
}

/** Whether Transcribe can be pressed for what `link_inspect` said, given
 *  whether yt-dlp is installed (null = still checking) and the opt-in. */
export function canTranscribeLink(info: LinkInfo | null, ytDlp: YtDlpStatus | null, allowLocal: boolean): boolean {
  if (!info || info.error || !info.kind) return false;
  if (info.kind === "platform" && ytDlp !== null && !ytDlp.found) return false;
  if (info.local && !allowLocal) return false;
  return true;
}

/** What blocks the link, in words, or null. */
export function linkProblem(info: LinkInfo | null, ytDlp: YtDlpStatus | null, allowLocal: boolean): string | null {
  if (!info) return null;
  if (info.error) return info.error;
  if (info.kind === "platform" && ytDlp !== null && !ytDlp.found) return "Links to video sites need yt-dlp, which was not found.";
  if (info.local && !allowLocal) return "This address is on this computer or your local network — tick “Allow local network addresses” to use it.";
  return null;
}
