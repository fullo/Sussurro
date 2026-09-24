import { formatBytes } from "./format";
import type { AudioFile, Item, ItemSummary } from "./types";

/** Saved audio (#141), pure helpers for the document pane and the Library. */

/** Total bytes of an item's saved audio (0 = none). */
export function audioBytes(item: Pick<Item, "audio">): number {
  return (item.audio ?? []).reduce((sum, f) => sum + (f.bytes || 0), 0);
}

/** Which channel a file holds, for display: `audio-remote.wav` → "remote";
 *  the single-channel `audio.wav` → "". */
export function audioChannel(file: AudioFile): string {
  const m = /^audio-([a-z]+)\.wav$/.exec(file.name);
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
