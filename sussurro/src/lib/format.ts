/* Pure formatting helpers shared by the classic UI and the workspace. */

/** 12 → "12", 1234 → "1,234", 15200 → "15.2k" */
export function fmtCount(n: number): string {
  if (n >= 10_000) return `${(n / 1000).toFixed(1)}k`;
  return n.toLocaleString("en-US");
}

/** File sizes: "812 B", "34 KB", "112.4 MB", "1.2 GB" (decimal units, like
 *  Finder and Explorer's defaults). */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 1000) return `${Math.max(0, Math.round(n || 0))} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1000;
  let i = 0;
  while (v >= 999.5 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return `${i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/** `HH:MM:SS` for a millisecond offset — same as `format_timestamp` in
 *  archive/render.rs, so the UI and transcript.md agree. */
export function formatTimestamp(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${pad2(Math.floor(s / 3600))}:${pad2(Math.floor(s / 60) % 60)}:${pad2(s % 60)}`;
}

/** Elapsed clock for a running session: `MM:SS`, or `H:MM:SS` past an hour. */
export function formatClock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const mm = pad2(Math.floor(s / 60) % 60);
  return h > 0 ? `${h}:${mm}:${pad2(s % 60)}` : `${mm}:${pad2(s % 60)}`;
}

/** Seconds from a frontmatter duration (`HH:MM:SS`, `MM:SS` or seconds);
 *  null when absent or unreadable. */
export function parseDuration(d: string | undefined | null): number | null {
  if (!d) return null;
  const parts = d.trim().split(":");
  if (parts.length > 3 || parts.some((p) => !/^\d+$/.test(p))) return null;
  return parts.reduce((acc, p) => acc * 60 + Number(p), 0);
}

/** Human length of an item: "45 s", "3 min", "1 h 12". */
export function formatDurationLabel(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return "";
  const s = Math.round(seconds);
  if (s < 60) return `${s} s`;
  const min = Math.round(s / 60);
  if (min < 60) return `${min} min`;
  const h = Math.floor(min / 60);
  const rest = min % 60;
  return rest === 0 ? `${h} h` : `${h} h ${pad2(rest)}`;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

function sameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

/** Library date: "Today 08:40", "Yesterday 18:02", "22 Sep", "22 Sep 2025"
 *  (local time). Unparseable dates are shown as written. */
export function formatItemDate(iso: string, now: Date = new Date()): string {
  const d = new Date(iso);
  if (!iso || Number.isNaN(d.getTime())) return iso;
  const time = `${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
  if (sameDay(d, now)) return `Today ${time}`;
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (sameDay(d, yesterday)) return `Yesterday ${time}`;
  const day = `${d.getDate()} ${MONTHS[d.getMonth()]}`;
  return d.getFullYear() === now.getFullYear() ? day : `${day} ${d.getFullYear()}`;
}

/** Full date for the document header: "24 Sep 2026, 10:00". */
export function formatLongDate(iso: string): string {
  const d = new Date(iso);
  if (!iso || Number.isNaN(d.getTime())) return iso;
  return `${d.getDate()} ${MONTHS[d.getMonth()]} ${d.getFullYear()}, ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

/** Percentage (0–100, rounded) of a file processed; 0 when the total is unknown. */
export function progressPercent(processed: number, total: number): number {
  if (!(total > 0)) return 0;
  return Math.max(0, Math.min(100, Math.round((processed / total) * 100)));
}

/** Last path component of a file path, for either separator. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

/** "Finder" / "Explorer" / "file manager", for "Reveal in …" buttons. */
export function fileManagerName(platform: string = typeof navigator !== "undefined" ? navigator.userAgent : ""): string {
  if (/Mac/i.test(platform)) return "Finder";
  if (/Win/i.test(platform)) return "Explorer";
  return "file manager";
}
