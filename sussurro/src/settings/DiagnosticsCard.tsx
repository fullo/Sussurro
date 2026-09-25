import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Tip } from "../components/ui";
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
  type DiagnosticsSnapshot,
  type TimingSample,
} from "../lib/diagnostics";
import type { CardProps } from "./DictationCard";

/** How often the panel asks for fresh numbers while it is open. */
const POLL_MS = 1000;

/** Settings → Diagnostics (#101): live timings, engine and backend,
 *  memory, CPU, sidecars, cleanup latency and long-form backlog. Polls
 *  only while mounted (the section is open) and the window is visible. */
export function DiagnosticsCard({ ctl }: CardProps) {
  const [snap, setSnap] = useState<DiagnosticsSnapshot | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      if (document.visibilityState === "visible") {
        try {
          const s = await invoke<DiagnosticsSnapshot>("diagnostics_snapshot");
          if (alive) {
            setSnap(s);
            setError("");
          }
        } catch (e) {
          if (alive) setError(String(e));
        }
      }
      if (alive) timer = setTimeout(tick, POLL_MS);
    };
    tick();
    return () => {
      alive = false;
      if (timer) clearTimeout(timer);
    };
  }, []);

  return (
    <Card
      title="Diagnostics"
      headerExtra={
        <button
          type="button"
          className="btn-ghost"
          title="Copy version, configuration and these numbers to the clipboard for a bug report — no transcripts, no keys, your home folder removed"
          onClick={ctl.copyDiagnostics}
        >
          Copy diagnostics
        </button>
      }
    >
      <p className="card-hint">
        Live while this section is open. The numbers stay in memory on this computer for this session — nothing is
        saved or sent.
      </p>
      {error && <p className="field-err">Diagnostics unavailable: {error}</p>}
      {!snap && !error && <p className="card-hint">Reading…</p>}
      {snap && <DiagnosticsBody snap={snap} />}
    </Card>
  );
}

function Row({ label, tip, value, sub }: { label: string; tip?: string; value: ReactNode; sub?: string }) {
  return (
    <div className="field diag-row">
      <div className="field-label">
        <span>
          {label} {tip && <Tip text={tip} />}
        </span>
        {sub && <small>{sub}</small>}
      </div>
      <div className="diag-value">{value}</div>
    </div>
  );
}

function DiagnosticsBody({ snap }: { snap: DiagnosticsSnapshot }) {
  const last = snap.dictations[snap.dictations.length - 1];
  const rows = statRows(snap.dictation_summary, snap.segment_summary);
  const qwen = snap.stt.sidecar;
  const cleanupTarget =
    snap.cleanup.api === "bundled"
      ? "bundled model on this computer"
      : `${snap.cleanup.api === "ollama" ? "Ollama" : "OpenAI-compatible"} · ${snap.cleanup.endpoint}${snap.cleanup.external ? " (external)" : ""}`;

  return (
    <>
      <h3 className="diag-h">Last dictation</h3>
      {last ? (
        <LastDictation sample={last} />
      ) : (
        <p className="card-hint">No dictation yet in this session — dictate with the shortcut and the timings appear here.</p>
      )}

      <h3 className="diag-h">
        Recent runs{" "}
        <Tip text="Median and 90th percentile (p90: 9 runs in 10 were at least this fast) over the last 20 dictations and the last 20 long-form segments (mic, file, link and meeting runs). Total is Finish → Idle for a dictation, speech-to-text plus cleanup for a segment. Real-time factor = speech-to-text time ÷ audio length: below 1 is faster than real time." />
      </h3>
      {rows.length > 0 ? (
        <>
          {snap.dictations.length > 1 && (
            <>
              <Sparkline samples={snap.dictations} />
              <small className="diag-caption">Finish → Idle of the last {snap.dictations.length} dictations, oldest first</small>
            </>
          )}
          <table className="diag-table">
            <thead>
              <tr>
                <th scope="col">Phase</th>
                <th scope="col">Dictations ({snap.dictation_summary.count})</th>
                <th scope="col">Long-form segments ({snap.segment_summary.count})</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.label}>
                  <th scope="row">{r.label}</th>
                  <td>{r.dictations}</td>
                  <td>{r.segments}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      ) : (
        <p className="card-hint">Nothing timed yet.</p>
      )}

      <h3 className="diag-h">Speech engine</h3>
      <Row
        label="Engine and model"
        value={`${engineName(snap.stt.engine)} · ${snap.stt.model}`}
        sub={sttStateLabel(snap.stt.state)}
      />
      <Row
        label="Runs on"
        tip="What the engine actually set up, read from its own log — not guessed from how the app was built. A GPU build still falls back to the CPU when the GPU is missing or fails to start."
        value={<strong>{backendLabel(snap.stt.backend)}</strong>}
        sub={backendSource(snap.stt.backend)}
      />
      <Row label="Model file" value={fmtMemory(snap.stt.model_file_bytes)} sub="size on disk" />
      {qwen && <Row label="Qwen3-ASR server" value={sidecarLabel(qwen)} sub="llama-server sidecar" />}

      <h3 className="diag-h">This computer</h3>
      <Row
        label="App memory"
        tip="What the operating system counts for Sussurro right now (macOS: memory footprint, as in Activity Monitor; Windows: working set; Linux: resident memory). A loaded model is most of it; the model is unloaded after 15 minutes unused."
        value={fmtMemory(snap.process.memory_bytes)}
      />
      <Row
        label="App CPU"
        value={fmtPercent(snap.process.cpu_percent)}
        sub={`of all ${snap.process.cpus} logical cores, over the last second`}
      />
      <Row
        label="Bundled LLM server"
        value={sidecarLabel(snap.bundled_llm)}
        sub={snap.bundled_llm.running ? backendLabel(snap.bundled_llm.backend) : "starts when the Local (bundled) profile is used"}
      />

      <h3 className="diag-h">Cleanup</h3>
      <Row label="Profile" value={snap.cleanup.profile} sub={snap.cleanup.active ? cleanupTarget : "cleanup is off"} />
      <Row label="Last call" tip="How long the cleanup server took to answer the last request, from any dictation or run." value={lastCallLabel(snap.cleanup.last_call)} />

      <h3 className="diag-h">Long-form backlog</h3>
      {snap.backlog.length === 0 ? (
        <p className="card-hint">No run in progress.</p>
      ) : (
        snap.backlog.map((b) => (
          <Row key={b.session_id} label={`Run ${b.session_id}`} value={backlogLabel(b)} sub={`${b.processed_s.toFixed(0)} s of audio done`} />
        ))
      )}
    </>
  );
}

function LastDictation({ sample }: { sample: TimingSample }) {
  const slices = phaseSlices(sample);
  return (
    <div className="diag-last">
      <div className="diag-total">
        <strong>{fmtMs(sample.total_ms)}</strong> from Finish to Idle
        {sample.audio_ms != null && <> · {fmtMs(sample.audio_ms)} recorded</>}
        {sample.failed && <> · ended with an error</>}
      </div>
      {slices.length > 0 && (
        <div className="diag-bar" role="img" aria-label={slices.map((s) => `${s.label} ${fmtMs(s.ms)}`).join(", ")}>
          {slices.map((s) => (
            <span
              key={s.key}
              className="diag-seg"
              style={{ width: `${s.pct}%`, background: s.color }}
              title={`${s.label}: ${fmtMs(s.ms)} (${Math.round(s.pct)} %)`}
            />
          ))}
        </div>
      )}
      <ul className="diag-legend">
        {slices.map((s) => (
          <li key={s.key}>
            <span className="diag-swatch" style={{ background: s.color }} aria-hidden="true" />
            {s.label} <span className="diag-num">{fmtMs(s.ms)}</span>
          </li>
        ))}
        {sample.cleanup_ms == null && <li className="diag-muted">No cleanup</li>}
      </ul>
      {sample.audio_ms != null && sample.stt_ms != null && sample.audio_ms > 0 && (
        <p className="card-hint">Speech-to-text speed: {fmtRealtime(sample.stt_ms / sample.audio_ms)}</p>
      )}
    </div>
  );
}

function Sparkline({ samples }: { samples: TimingSample[] }) {
  const heights = sparkline(samples);
  return (
    <div className="diag-spark" role="img" aria-label={`Finish to Idle of the last ${samples.length} dictations`}>
      {samples.map((s, i) => (
        <span
          key={`${s.at_ms}-${i}`}
          className={`diag-spark-bar${s.failed ? " failed" : ""}`}
          style={{ height: `${Math.max(heights[i], 3)}%` }}
          title={`${new Date(s.at_ms).toLocaleTimeString()} · ${s.failed ? "failed" : fmtMs(s.total_ms)}`}
        />
      ))}
    </div>
  );
}
