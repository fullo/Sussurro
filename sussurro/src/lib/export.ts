/* Pure helpers for the Export section of the context pane (#133). */

import type { ExportFormat, ItemType, SubtitlesMode, SubtitlesStatus } from "./types";

export interface ExportChoice {
  format: ExportFormat;
  /** Button label (`.srt`). */
  label: string;
  /** Save-dialog filter name. */
  filter: string;
  title: string;
}

const CHOICES: ExportChoice[] = [
  { format: "md", label: ".md", filter: "Markdown", title: "The transcript as stored, with its frontmatter" },
  { format: "txt", label: ".txt", filter: "Plain text", title: "Plain text, one timestamped line per segment" },
  { format: "srt", label: ".srt", filter: "SubRip subtitles", title: "Subtitles for video players (SubRip)" },
  { format: "vtt", label: ".vtt", filter: "WebVTT subtitles", title: "Subtitles for the web (WebVTT)" },
];

/** Whether items of this type have subtitles: meetings and transcriptions
 *  do, notes never (P10, P11). */
export function hasSubtitles(type: ItemType): boolean {
  return type !== "note";
}

/** The export formats offered for an item: `.srt`/`.vtt` are hidden for
 *  notes. */
export function exportChoices(type: ItemType): ExportChoice[] {
  return CHOICES.filter((c) => hasSubtitles(type) || (c.format !== "srt" && c.format !== "vtt"));
}

/** Suggested file name: the item's folder name plus the extension
 *  (`2026-09-24-weekly-sync.srt`). */
export function exportFileName(itemId: string, format: ExportFormat): string {
  const base = itemId.split("/").filter(Boolean).pop() || "sussurro";
  return `${base}.${format}`;
}

/** The subtitles line of the Export section: what `transcript.srt` is and
 *  whether a "Create .srt" button is offered (with its label). `null` for
 *  notes, which have no subtitles. */
export function subtitlesInfo(
  type: ItemType,
  mode: SubtitlesMode,
  status: SubtitlesStatus | null,
  recording: boolean,
): { note: string; action: string | null } | null {
  if (!hasSubtitles(type)) return null;
  if (recording) return { note: "Subtitles can be created when the recording ends.", action: null };
  if (status?.edited_externally) {
    return {
      note: `${status.file} was edited outside Sussurro, so it is kept as is. Export → .srt saves a fresh copy elsewhere.`,
      action: null,
    };
  }
  const exists = !!status?.exists;
  if (mode === "always") {
    return {
      note: exists
        ? "transcript.srt is next to the transcript and is updated every time it is saved."
        : "transcript.srt is written every time the transcript is saved.",
      action: exists ? null : "Create .srt",
    };
  }
  return {
    note: exists ? "transcript.srt is next to the transcript." : "No transcript.srt yet: create it when you need it.",
    action: exists ? "Update .srt" : "Create .srt",
  };
}
