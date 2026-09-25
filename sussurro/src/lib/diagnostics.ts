/* Settings → Diagnostics (#101): the shapes `diagnostics_snapshot` returns
   and the pure formatting the panel uses. Numbers only — the backend never
   sends text, keys or paths here. */

import { formatBytes } from "./format";

/** One timed dictation or long-form segment; durations in ms, null = the
 *  phase did not run. */
export type TimingSample = {
  at_ms: number;
  audio_ms: number | null;
  load_ms: number | null;
  stt_ms: number | null;
  cleanup_ms: number | null;
  paste_ms: number | null;
  total_ms: number | null;
  failed: boolean;
};

export type Stat = { median: number; p90: number; count: number };

export type TimingSummary = {
  count: number;
  total: Stat | null;
  load: Stat | null;
  stt: Stat | null;
  cleanup: Stat | null;
  paste: Stat | null;
  realtime_factor: Stat | null;
};

export type ComputeBackend = {
  kind: string;
  device: string | null;
  source: string;
  note: string | null;
};

export type SidecarStatus = {
  available: boolean;
  running: boolean;
  memory_bytes: number | null;
  backend: ComputeBackend | null;
  failures: number;
  model_file_bytes: number | null;
};

export type DiagnosticsSnapshot = {
  version: string;
  os: string;
  dictations: TimingSample[];
  dictation_summary: TimingSummary;
  segments: TimingSample[];
  segment_summary: TimingSummary;
  stt: {
    engine: "whisper" | "parakeet" | "qwen3_asr" | string;
    model: string;
    state: "not_loaded" | "loaded" | "busy" | string;
    backend: ComputeBackend | null;
    model_file_bytes: number | null;
    sidecar: SidecarStatus | null;
  };
  cleanup: {
    active: boolean;
    profile: string;
    api: "ollama" | "openai" | "bundled" | string;
    endpoint: string;
    external: boolean;
    last_call: { ms: number; ok: boolean; age_s: number } | null;
  };
  bundled_llm: SidecarStatus;
  process: { memory_bytes: number | null; cpu_percent: number | null; cpus: number };
  backlog: {
    session_id: number;
    backlog_s: number;
    processed_s: number;
    queue_len: number;
    segments_done: number;
  }[];
  recording: boolean;
};

/** 812 → "812 ms", 1450 → "1.4 s", 125000 → "2 min 5 s" (same as the
 *  Rust report). */
export function fmtMs(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return "—";
  const v = Math.max(0, Math.round(ms));
  if (v < 1000) return `${v} ms`;
  if (v < 60_000) return `${(v / 1000).toFixed(1)} s`;
  return `${Math.floor(v / 60_000)} min ${Math.floor((v % 60_000) / 1000)} s`;
}

export function fmtMemory(bytes: number | null | undefined): string {
  return bytes == null ? "—" : formatBytes(bytes);
}

/** 12.4 → "12 %", 0.3 → "0.3 %" (idle readings stay visible). */
export function fmtPercent(p: number | null | undefined): string {
  if (p == null || !Number.isFinite(p)) return "measuring…";
  return p < 1 ? `${p.toFixed(1)} %` : `${Math.round(p)} %`;
}

/** Real-time factor: 0.08 → "0.08× (12× faster than real time)". */
export function fmtRealtime(rtf: number | null | undefined): string {
  if (rtf == null || !Number.isFinite(rtf) || rtf <= 0) return "—";
  const speed = 1 / rtf;
  return speed >= 1
    ? `${rtf.toFixed(2)}× (${speed >= 10 ? Math.round(speed) : speed.toFixed(1)}× faster than real time)`
    : `${rtf.toFixed(2)}× (slower than real time)`;
}

/** The phases of a dictation, in order, as the stacked bar draws them. */
export const PHASES = [
  { key: "load_ms", label: "Model wait", color: "#2a78d6" },
  { key: "stt_ms", label: "Speech-to-text", color: "#eb6834" },
  { key: "cleanup_ms", label: "Cleanup", color: "#1baf7a" },
  { key: "paste_ms", label: "Paste", color: "#eda100" },
] as const;

export type PhaseKey = (typeof PHASES)[number]["key"];

export type PhaseSlice = {
  key: PhaseKey | "other";
  label: string;
  color: string;
  ms: number;
  /** Share of Finish → Idle, 0..100. */
  pct: number;
};

/** The slices of a dictation's Finish → Idle bar: each phase that ran,
 *  plus "Other" for the rest (reading the audio, silence trimming, event
 *  hops). Phases under 1 ms are left out. Pure. */
export function phaseSlices(s: TimingSample): PhaseSlice[] {
  const measured = PHASES.map((p) => ({ ...p, ms: s[p.key] ?? 0 })).filter((p) => p.ms >= 1);
  const sum = measured.reduce((a, p) => a + p.ms, 0);
  const total = Math.max(s.total_ms ?? 0, sum);
  if (total <= 0) return [];
  const slices: PhaseSlice[] = measured.map((p) => ({
    key: p.key,
    label: p.label,
    color: p.color,
    ms: p.ms,
    pct: (p.ms / total) * 100,
  }));
  const other = total - sum;
  if (other >= 1) {
    slices.push({ key: "other", label: "Other", color: "var(--border-strong)", ms: other, pct: (other / total) * 100 });
  }
  return slices;
}

export type StatRow = { label: string; dictations: string; segments: string };

function statCell(st: Stat | null, fmt: (v: number) => string = fmtMs): string {
  if (!st) return "—";
  return `${fmt(st.median)} · p90 ${fmt(st.p90)}`;
}

/** The history table: one row per phase, median · p90 for the last
 *  dictations and long-form segments. Rows empty in both are left out.
 *  Pure. */
export function statRows(d: TimingSummary, s: TimingSummary): StatRow[] {
  const rtf = (v: number) => `${v.toFixed(2)}×`;
  const rows: StatRow[] = [
    { label: "Total", dictations: statCell(d.total), segments: statCell(s.total) },
    { label: "Model wait", dictations: statCell(d.load), segments: statCell(s.load) },
    { label: "Speech-to-text", dictations: statCell(d.stt), segments: statCell(s.stt) },
    { label: "Cleanup", dictations: statCell(d.cleanup), segments: statCell(s.cleanup) },
    { label: "Paste", dictations: statCell(d.paste), segments: statCell(s.paste) },
    { label: "Real-time factor", dictations: statCell(d.realtime_factor, rtf), segments: statCell(s.realtime_factor, rtf) },
  ];
  return rows.filter((r) => r.dictations !== "—" || r.segments !== "—");
}

/** "Metal · Apple M1 Pro", "CPU · ONNX Runtime", "Vulkan · Vulkan0". */
export function backendLabel(b: ComputeBackend | null | undefined): string {
  if (!b) return "not known yet";
  const parts = [b.kind];
  if (b.device) parts.push(b.device);
  return parts.join(" · ");
}

/** Where the backend reading comes from, for the small print. */
export function backendSource(b: ComputeBackend | null | undefined): string {
  if (!b) return "shown after the first transcription";
  const parts = [b.note, `from the ${b.source}`].filter(Boolean);
  return parts.join(" · ");
}

export function engineName(engine: string): string {
  switch (engine) {
    case "whisper":
      return "Whisper";
    case "parakeet":
      return "Parakeet";
    case "qwen3_asr":
      return "Qwen3-ASR";
    default:
      return engine;
  }
}

export function sttStateLabel(state: string): string {
  switch (state) {
    case "loaded":
      return "loaded, idle";
    case "busy":
      return "working now";
    case "not_loaded":
      return "not loaded (loads on the next dictation)";
    default:
      return state;
  }
}

/** One line for a sidecar. Pure. */
export function sidecarLabel(s: SidecarStatus): string {
  if (!s.available) return "not in this build";
  if (!s.running) return s.failures > 0 ? `stopped · ${s.failures} crash(es)` : "stopped";
  const parts = ["running"];
  if (s.memory_bytes != null) parts.push(formatBytes(s.memory_bytes));
  if (s.failures > 0) parts.push(`${s.failures} crash(es)`);
  return parts.join(" · ");
}

/** "640 ms · ok · 3 s ago". Pure. */
export function lastCallLabel(c: DiagnosticsSnapshot["cleanup"]["last_call"]): string {
  if (!c) return "no call yet";
  const age = c.age_s < 60 ? `${c.age_s} s ago` : c.age_s < 3600 ? `${Math.floor(c.age_s / 60)} min ago` : `${Math.floor(c.age_s / 3600)} h ago`;
  return `${fmtMs(c.ms)} · ${c.ok ? "ok" : "failed"} · ${age}`;
}

/** "4.2 s behind · 1 queued · 5 done". Pure. */
export function backlogLabel(b: DiagnosticsSnapshot["backlog"][number]): string {
  const behind = b.backlog_s < 0.05 ? "keeping up" : `${b.backlog_s.toFixed(1)} s behind`;
  return `${behind} · ${b.queue_len} queued · ${b.segments_done} done`;
}

/** Bar heights (0..100) for the Finish → Idle sparkline, scaled to the
 *  slowest of the samples. Failed runs and runs without a total are 0. */
export function sparkline(samples: TimingSample[]): number[] {
  const totals = samples.map((s) => (s.failed ? 0 : s.total_ms ?? 0));
  const max = Math.max(0, ...totals);
  return max > 0 ? totals.map((t) => (t / max) * 100) : totals.map(() => 0);
}
