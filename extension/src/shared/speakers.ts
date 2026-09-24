/* Speaker chips in the side panel (#129): the same labels and colours the
 * app gives its speakers (#130, `sussurro/src-tauri/src/speakers/doc.rs`),
 * so a line looks the same in the panel and in the app. The app sends only
 * a speaker's id on each segment (and, from #131, `speaker {id, label}`);
 * everything else is derived here. `speakers.test.ts` checks the palette
 * against the Rust constants. Pure. */

/** The user's own microphone in a browser meeting (`YOU_ID`). */
export const YOU_ID = "you";
/** `YOU_COLOR`. */
export const YOU_COLOR = "#1a1a1a";
/** `VOICE_PREFIX`: the acoustic voices are `voice:1`, `voice:2`, … */
export const VOICE_PREFIX = "voice:";
/** `VOICE_COLORS`, cycled by voice number. */
export const VOICE_COLORS: readonly string[] = ["#0f766e", "#7e22ce", "#1f6feb", "#c2410c", "#be185d", "#4d7c0f", "#0369a1", "#9a3412"];

export interface ChipSpeaker {
  id: string;
  label: string;
  color: string;
}

/** What the app told about a speaker (`speaker {id, label, color?}`). */
export interface KnownSpeaker {
  label: string;
  color?: string;
}

/** `voice:<n>` → n (n ≥ 1). */
export function voiceNumber(id: string): number | null {
  if (!id.startsWith(VOICE_PREFIX)) return null;
  const rest = id.slice(VOICE_PREFIX.length);
  if (!/^\d+$/.test(rest)) return null;
  const n = Number(rest);
  return n > 0 ? n : null;
}

/** `voice_color(n)`. */
export function voiceColor(n: number): string {
  return VOICE_COLORS[(Math.max(1, n) - 1) % VOICE_COLORS.length];
}

/** A stable palette colour for any other id (e.g. a Meet name, #131). */
function hashedColor(id: string): string {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return VOICE_COLORS[h % VOICE_COLORS.length];
}

/** A label when the app sent none: `meet:Anna` → `Anna`. */
function defaultLabel(id: string): string {
  const n = voiceNumber(id);
  if (n !== null) return `Voice ${n}`;
  if (id === YOU_ID) return "You";
  const colon = id.indexOf(":");
  const name = colon >= 0 ? id.slice(colon + 1) : id;
  return name.trim() || id;
}

function defaultColor(id: string): string {
  if (id === YOU_ID) return YOU_COLOR;
  const n = voiceNumber(id);
  return n !== null ? voiceColor(n) : hashedColor(id);
}

/** The chip for speaker `id`, from what the app said about it, else the
 *  app's own defaults. */
export function chipFor(id: string, known?: KnownSpeaker): ChipSpeaker {
  const label = known?.label?.trim() || defaultLabel(id);
  const color = known?.color && /^#[0-9a-fA-F]{3,8}$/.test(known.color) ? known.color : defaultColor(id);
  return { id, label, color };
}

/** Whose line it is: its speaker, else (no speakers on the run) the mic
 *  channel is the user — as in the app, a browser meeting's mic is "You". */
export function lineSpeakerId(line: { speaker_id?: string; channel?: string }): string | undefined {
  return line.speaker_id || (line.channel === "mic" ? YOU_ID : undefined);
}
