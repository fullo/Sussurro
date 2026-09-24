/* Options page: pairing with the Sussurro app (#127, E6). Paste the pairing
   code from Sussurro → Settings → Browser extension (or type port and token),
   save it to storage.local, and "Test connection" (GET /app/version with the
   token). Once saved, the token is only ever shown masked. */
import { StrictMode, useState, type FormEvent, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { Header } from "../shared/Header";
import { describeResult, testConnection, type ResultMessage } from "../shared/connection";
import {
  DEFAULT_PORT,
  clearPairing,
  maskToken,
  parsePairingCode,
  setPairing,
  validatePairing,
  type Pairing,
} from "../shared/pairing";
import { usePairing } from "../shared/usePairing";
import "../shared/page.css";

function Options() {
  const saved = usePairing();
  const [editing, setEditing] = useState(false);
  const [result, setResult] = useState<ResultMessage | null>(null);
  const [testing, setTesting] = useState(false);

  const runTest = async (p: Pairing) => {
    setTesting(true);
    setResult(null);
    try {
      setResult(describeResult(await testConnection(p), p.port));
    } finally {
      setTesting(false);
    }
  };

  const onSaved = (p: Pairing) => {
    setEditing(false);
    void runTest(p);
  };

  if (saved === undefined) return <Page />;

  return (
    <Page>
      {saved && !editing ? (
        <section className="panel" aria-labelledby="paired-title">
          <h2 id="paired-title">Paired with Sussurro</h2>
          <dl className="kv">
            <dt>Address</dt>
            <dd>127.0.0.1:{saved.port}</dd>
            <dt>Token</dt>
            <dd aria-label="Token, hidden">{maskToken(saved.token)}</dd>
          </dl>
          <div className="actions">
            <button type="button" className="btn primary" disabled={testing} onClick={() => runTest(saved)}>
              {testing ? "Testing…" : "Test connection"}
            </button>
            <button
              type="button"
              className="btn"
              onClick={() => {
                setResult(null);
                setEditing(true);
              }}
            >
              Pair again
            </button>
            <button
              type="button"
              className="btn"
              onClick={async () => {
                setResult(null);
                await clearPairing();
              }}
            >
              Forget
            </button>
          </div>
          {result && <Result message={result} />}
        </section>
      ) : (
        <PairForm
          initialPort={saved?.port ?? DEFAULT_PORT}
          onSaved={onSaved}
          onCancel={saved ? () => setEditing(false) : undefined}
        />
      )}
    </Page>
  );
}

function Page({ children }: { children?: ReactNode }) {
  return (
    <main className="page options">
      <Header title="Sussurro options" />
      {children}
    </main>
  );
}

function PairForm({
  initialPort,
  onSaved,
  onCancel,
}: {
  initialPort: number;
  onSaved: (p: Pairing) => void;
  onCancel?: () => void;
}) {
  const [manual, setManual] = useState(false);
  const [code, setCode] = useState("");
  const [port, setPort] = useState(String(initialPort));
  const [token, setToken] = useState("");
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const parsed = manual ? validatePairing(port, token) : parsePairingCode(code);
    if (!parsed.ok) {
      setError(parsed.error);
      return;
    }
    setSaving(true);
    try {
      const p = await setPairing(parsed.value);
      // The token leaves the form as soon as it is stored.
      setCode("");
      setToken("");
      setError("");
      onSaved(p);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save the pairing.");
    } finally {
      setSaving(false);
    }
  };

  return (
    <form className="panel" onSubmit={submit} aria-labelledby="pair-title" autoComplete="off">
      <h2 id="pair-title">Pair with the Sussurro app</h2>
      <ol className="steps">
        <li>In Sussurro, open Settings → Browser extension and turn on Meetings.</li>
        <li>Click Copy pairing code.</li>
        <li>Paste it here and save.</li>
      </ol>
      {!manual ? (
        <label className="field">
          <span>Pairing code</span>
          <textarea
            value={code}
            onChange={(e) => setCode(e.target.value)}
            placeholder="sussurro:4525:…"
            rows={3}
            spellCheck={false}
            autoFocus
            aria-invalid={error ? true : undefined}
          />
        </label>
      ) : (
        <div className="field-row">
          <label className="field port">
            <span>Port</span>
            <input
              value={port}
              onChange={(e) => setPort(e.target.value)}
              inputMode="numeric"
              aria-invalid={error ? true : undefined}
            />
          </label>
          <label className="field grow">
            <span>Token</span>
            <input
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              spellCheck={false}
              aria-invalid={error ? true : undefined}
            />
          </label>
        </div>
      )}
      {error && (
        <p className="result error" role="alert">
          {error}
        </p>
      )}
      <div className="actions">
        <button type="submit" className="btn primary" disabled={saving}>
          {saving ? "Saving…" : "Save and test"}
        </button>
        <button
          type="button"
          className="btn link"
          onClick={() => {
            setManual(!manual);
            setError("");
          }}
        >
          {manual ? "Paste a pairing code instead" : "Enter port and token separately"}
        </button>
        {onCancel && (
          <button type="button" className="btn" onClick={onCancel}>
            Cancel
          </button>
        )}
      </div>
    </form>
  );
}

function Result({ message }: { message: ResultMessage }) {
  return (
    <div className={`result ${message.ok ? "ok" : "error"}`} role="status">
      <strong>{message.title}</strong>
      <p>{message.detail}</p>
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Options />
  </StrictMode>,
);
