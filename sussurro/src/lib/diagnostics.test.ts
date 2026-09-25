import { describe, expect, it } from "vitest";
import {
  backendLabel,
  backendSource,
  backlogLabel,
  engineName,
  fmtMemory,
  fmtMs,
  fmtPercent,
  fmtRealtime,
  lastCallLabel,
  phaseSlices,
  sidecarLabel,
  sparkline,
  statRows,
  sttStateLabel,
  type TimingSample,
  type TimingSummary,
} from "./diagnostics";

const sample = (over: Partial<TimingSample> = {}): TimingSample => ({
  at_ms: 0,
  audio_ms: null,
  load_ms: null,
  stt_ms: null,
  cleanup_ms: null,
  paste_ms: null,
  total_ms: null,
  failed: false,
  ...over,
});

const empty: TimingSummary = {
  count: 0,
  total: null,
  load: null,
  stt: null,
  cleanup: null,
  paste: null,
  realtime_factor: null,
};

describe("fmtMs", () => {
  it("matches the Rust report", () => {
    expect(fmtMs(0)).toBe("0 ms");
    expect(fmtMs(812)).toBe("812 ms");
    expect(fmtMs(812.6)).toBe("813 ms");
    expect(fmtMs(1450)).toBe("1.4 s");
    expect(fmtMs(125_000)).toBe("2 min 5 s");
    expect(fmtMs(null)).toBe("—");
  });
});

describe("small labels", () => {
  it("formats memory, CPU and real-time factor", () => {
    expect(fmtMemory(812_000_000)).toBe("812.0 MB");
    expect(fmtMemory(null)).toBe("—");
    expect(fmtPercent(12.4)).toBe("12 %");
    expect(fmtPercent(0.34)).toBe("0.3 %");
    expect(fmtPercent(null)).toBe("measuring…");
    expect(fmtRealtime(0.08)).toBe("0.08× (13× faster than real time)");
    expect(fmtRealtime(0.5)).toBe("0.50× (2.0× faster than real time)");
    expect(fmtRealtime(1.5)).toBe("1.50× (slower than real time)");
    expect(fmtRealtime(null)).toBe("—");
  });

  it("names engines and states", () => {
    expect(engineName("qwen3_asr")).toBe("Qwen3-ASR");
    expect(engineName("whisper")).toBe("Whisper");
    expect(sttStateLabel("busy")).toBe("working now");
    expect(sttStateLabel("not_loaded")).toMatch(/not loaded/);
  });

  it("describes the backend and where the reading comes from", () => {
    const metal = { kind: "Metal", device: "Apple M1 Pro", source: "whisper.cpp log", note: "with BLAS" };
    expect(backendLabel(metal)).toBe("Metal · Apple M1 Pro");
    expect(backendSource(metal)).toBe("with BLAS · from the whisper.cpp log");
    expect(backendLabel({ kind: "CPU", device: null, source: "engine", note: null })).toBe("CPU");
    expect(backendLabel(null)).toBe("not known yet");
    expect(backendSource(null)).toMatch(/first transcription/);
  });

  it("describes sidecars", () => {
    const base = { available: true, running: true, memory_bytes: 1_900_000_000, backend: null, failures: 0, model_file_bytes: null };
    expect(sidecarLabel(base)).toBe("running · 1.9 GB");
    expect(sidecarLabel({ ...base, running: false })).toBe("stopped");
    expect(sidecarLabel({ ...base, running: false, failures: 2 })).toBe("stopped · 2 crash(es)");
    expect(sidecarLabel({ ...base, available: false })).toBe("not in this build");
  });

  it("describes the last cleanup call and the backlog", () => {
    expect(lastCallLabel({ ms: 640, ok: true, age_s: 3 })).toBe("640 ms · ok · 3 s ago");
    expect(lastCallLabel({ ms: 30_000, ok: false, age_s: 125 })).toBe("30.0 s · failed · 2 min ago");
    expect(lastCallLabel(null)).toBe("no call yet");
    const b = { session_id: 1, backlog_s: 4.2, processed_s: 10, queue_len: 1, segments_done: 5 };
    expect(backlogLabel(b)).toBe("4.2 s behind · 1 queued · 5 done");
    expect(backlogLabel({ ...b, backlog_s: 0 })).toBe("keeping up · 1 queued · 5 done");
  });
});

describe("phaseSlices", () => {
  it("splits Finish → Idle into the phases that ran plus the rest", () => {
    const s = sample({ load_ms: 0, stt_ms: 600, cleanup_ms: 300, paste_ms: null, total_ms: 1000 });
    const slices = phaseSlices(s);
    expect(slices.map((x) => [x.key, x.ms, Math.round(x.pct)])).toEqual([
      ["stt_ms", 600, 60],
      ["cleanup_ms", 300, 30],
      ["other", 100, 10],
    ]);
  });

  it("never exceeds 100 % when the phases outrun the total", () => {
    const slices = phaseSlices(sample({ stt_ms: 800, paste_ms: 400, total_ms: 1000 }));
    expect(slices.reduce((a, x) => a + x.pct, 0)).toBeCloseTo(100);
    expect(slices.some((x) => x.key === "other")).toBe(false);
  });

  it("is empty without timings", () => {
    expect(phaseSlices(sample())).toEqual([]);
  });
});

describe("statRows", () => {
  it("shows median and p90 per phase, dropping rows empty on both sides", () => {
    const d: TimingSummary = {
      ...empty,
      count: 3,
      total: { median: 1200, p90: 2100, count: 3 },
      stt: { median: 500, p90: 800, count: 3 },
      realtime_factor: { median: 0.12, p90: 0.2, count: 3 },
    };
    const s: TimingSummary = { ...empty, count: 2, stt: { median: 1500, p90: 1800, count: 2 } };
    expect(statRows(d, s)).toEqual([
      { label: "Total", dictations: "1.2 s · p90 2.1 s", segments: "—" },
      { label: "Speech-to-text", dictations: "500 ms · p90 800 ms", segments: "1.5 s · p90 1.8 s" },
      { label: "Real-time factor", dictations: "0.12× · p90 0.20×", segments: "—" },
    ]);
    expect(statRows(empty, empty)).toEqual([]);
  });
});

describe("sparkline", () => {
  it("scales to the slowest run; failed runs are flat", () => {
    const got = sparkline([
      sample({ total_ms: 500 }),
      sample({ total_ms: 1000 }),
      sample({ total_ms: 9000, failed: true }),
      sample({ total_ms: null }),
    ]);
    expect(got).toEqual([50, 100, 0, 0]);
    expect(sparkline([])).toEqual([]);
  });
});
