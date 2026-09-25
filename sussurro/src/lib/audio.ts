import { formatBytes } from "./format";
import type { AudioFile, CompressProgress, CompressSummary, Item, ItemSummary, SavedAudioFormat, Settings } from "./types";

/** Saved audio (#141), pure helpers for the document pane and the Library. */

/** Total bytes of an item's saved audio (0 = none). */
export function audioBytes(item: Pick<Item, "audio">): number {
  return (item.audio ?? []).reduce((sum, f) => sum + (f.bytes || 0), 0);
}

/** The format new runs save their audio in (#247): Opus unless the
 *  settings say WAV — the backend's default for a new install (#248, P16;
 *  it pins WAV for settings from before 0.11 and always sends the key). */
export function savedAudioFormat(settings: Pick<Settings, "saved_audio_format">): SavedAudioFormat {
  return settings.saved_audio_format === "wav" ? "wav" : "opus";
}

/** The item's WAV files: what *Compress audio* converts (#248). */
export function wavFiles(item: Pick<Item, "audio">): AudioFile[] {
  return (item.audio ?? []).filter((f) => f.name.endsWith(".wav"));
}

/** A *Compress audio* job's progress, 0–100. */
export function compressPercent(p: Pick<CompressProgress, "done_bytes" | "total_bytes">): number {
  if (p.total_bytes <= 0) return 0;
  return Math.max(0, Math.min(100, Math.round((p.done_bytes / p.total_bytes) * 100)));
}

/** One line for a finished *Compress audio* job. */
export function compressSummaryText(s: CompressSummary): string {
  const failed = s.failed.length;
  const parts: string[] = [];
  if (s.files > 0) {
    const where = s.items > 1 ? ` in ${s.items} items` : "";
    parts.push(
      `Audio compressed${where}: ${formatBytes(s.bytes_before)} → ${formatBytes(s.bytes_after)}. The WAV ${s.files === 1 ? "file is" : "files are"} in the trash.`,
    );
  } else if (s.cancelled) {
    parts.push("Compression cancelled: the WAV audio is kept.");
  } else if (failed === 0) {
    parts.push("No WAV audio to compress.");
  }
  if (s.cancelled && s.files > 0) parts.push("Cancelled before the end: the rest stays WAV.");
  if (failed > 0) parts.push(`${failed} item${failed === 1 ? "" : "s"} could not be compressed and keep${failed === 1 ? "s" : ""} the WAV.`);
  return parts.join(" ");
}

/** Approximate size of one hour of saved audio, for the settings text. */
export const AUDIO_MB_PER_HOUR: Record<SavedAudioFormat, number> = { wav: 115, opus: 11 };

/** Which channel a file holds, for display: `audio-remote.wav` (or
 *  `.opus`) → "remote"; the single-channel `audio.wav` → "". */
export function audioChannel(file: AudioFile): string {
  const m = /^audio-([a-z]+)\.(?:wav|opus)$/.exec(file.name);
  if (!m) return "";
  return ({ mic: "You (microphone)", remote: "Others (remote)", system: "System audio", file: "File" } as Record<string, string>)[m[1]] ?? m[1];
}

/** "Audio · 112.4 MB" or "Audio · 2 files · 230 MB"; "" without audio. */
export function audioLabel(item: Pick<Item, "audio">): string {
  const files = item.audio ?? [];
  if (files.length === 0) return "";
  const size = formatBytes(audioBytes(item));
  return files.length === 1 ? `Audio · ${size}` : `Audio · ${files.length} files · ${size}`;
}

/** The Library row's marker: "♪ 112 MB"; "" without audio. */
export function audioBadge(summary: Pick<ItemSummary, "audio_bytes">): string {
  const b = summary.audio_bytes ?? 0;
  return b > 0 ? `♪ ${formatBytes(b)}` : "";
}
