import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Card, Switch, Tip } from "../components/ui";
import {
  archiveApiStatus,
  curlExample,
  scopeSummary,
  SCOPES,
  toggleScope,
  tokenNameError,
  type ArchiveScope,
  type ArchiveTokenInfo,
  type NewArchiveToken,
} from "../lib/archiveTokens";
import { formatItemDate } from "../lib/format";
import type { CardProps } from "./DictationCard";

/** Settings → Scripting (#249, E14): the local API for your own scripts —
 *  the token-less routes (#215) and the archive API with its scoped tokens.
 *  A token's plaintext is shown once, right after it is created; the
 *  backend keeps only its SHA-256 and the list never carries it. */
export function ScriptingCard({ ctl }: CardProps) {
  const { settings, save, setBusy, flash } = ctl;
  const [tokens, setTokens] = useState<ArchiveTokenInfo[]>([]);
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<ArchiveScope[]>(["read"]);
  const [created, setCreated] = useState<NewArchiveToken | null>(null);
  const [confirmRevoke, setConfirmRevoke] = useState<string | null>(null);
  const [working, setWorking] = useState(false);

  const refresh = () =>
    invoke<ArchiveTokenInfo[]>("archive_tokens_list")
      .then(setTokens)
      .catch((e) => setBusy(String(e)));
  useEffect(() => {
    refresh();
    // Last-used times move while scripts run: refresh when the card opens.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const nameError = name.trim() ? tokenNameError(name, tokens) : null;
  const canCreate = !working && !!name.trim() && !nameError && scopes.length > 0;

  const create = async () => {
    setWorking(true);
    try {
      const t = await invoke<NewArchiveToken>("archive_token_create", { name: name.trim(), scopes });
      setCreated(t);
      setName("");
      setScopes(["read"]);
      await refresh();
    } catch (e) {
      setBusy(String(e));
    } finally {
      setWorking(false);
    }
  };

  const revoke = async (id: string) => {
    setWorking(true);
    try {
      setTokens(await invoke<ArchiveTokenInfo[]>("archive_token_revoke", { id }));
      setConfirmRevoke(null);
      if (created?.info.id === id) setCreated(null);
      flash("Token revoked: scripts using it are refused from now on.");
    } catch (e) {
      setBusy(String(e));
    } finally {
      setWorking(false);
    }
  };

  const copyToken = async () => {
    if (!created) return;
    try {
      await invoke("copy_text", { text: created.token });
      flash("Token copied. Store it where your script reads it: Sussurro won't show it again.", 6000);
    } catch (e) {
      setBusy(String(e));
    }
  };

  return (
    <Card title={<>Scripting <span className="via">local API for your scripts</span></>}>
      <p className="card-hint">
        Your own scripts can use Sussurro through the local HTTP API on <code>127.0.0.1:{settings.api_port}</code> — this
        computer only. Web pages can't. The local API itself is switched on in Behavior → Advanced and applies when
        Sussurro restarts{settings.api_enabled ? "." : ": it is off now."}
      </p>

      <div className="field">
        <div className="field-label">
          <span>
            Scripting routes{" "}
            <Tip text="POST /clean (text → cleaned), POST /transcribe?ext=wav (audio file → transcript) and GET /history?q= — without a token, so any program on this computer can use them (web pages can't: other sites' requests are refused). Leave off unless your own scripts use them. curl examples in the README." />
          </span>
          <small>token-less /clean, /transcribe, /history · applies at once</small>
        </div>
        <Switch
          checked={settings.api_scripting}
          onChange={(v) => save({ ...settings, api_scripting: v })}
          label="Scripting routes"
        />
      </div>

      <div className="field">
        <div className="field-label">
          <span>
            Archive API{" "}
            <Tip text="Routes under /archive let a script list, search, read and export your Library, and add notes — each request with a token from below, sent as Authorization: Bearer. Every request from a browser is refused, extensions included. Off by default; applies at once." />
          </span>
          <small>{archiveApiStatus(settings.api_enabled, !!settings.api_archive, tokens.length)}</small>
        </div>
        <Switch
          checked={!!settings.api_archive}
          onChange={(v) => save({ ...settings, api_archive: v })}
          label="Archive API"
        />
      </div>

      {created && (
        <div className="field field-col" role="status">
          <div className="field-label">
            <span>New token “{created.info.name}”</span>
            <small>shown only now: copy it before leaving this page</small>
          </div>
          <div className="model-row">
            <span className="path" aria-label="New archive token">{created.token}</span>
            <button type="button" className="btn-ghost" onClick={copyToken}>
              Copy
            </button>
            <button type="button" className="btn-ghost" onClick={() => setCreated(null)}>
              Done
            </button>
          </div>
          <p className="card-hint">
            Try it: <code>{curlExample(settings.api_port, created.info.scopes)}</code> with the token in <code>SUSSURRO_TOKEN</code>. Treat it
            like a password; if it leaks, revoke it below and create another.
          </p>
        </div>
      )}

      <div className="field field-col">
        <div className="field-label">
          <span>
            Archive tokens{" "}
            <Tip text="One token per script, so you can revoke one without touching the others. Sussurro stores only a fingerprint (SHA-256) of each token: it can't show a token again, and nothing on disk can be used in its place." />
          </span>
          <small>{tokens.length ? `${tokens.length} token${tokens.length === 1 ? "" : "s"}` : "none yet"}</small>
        </div>
        {tokens.length > 0 && (
          <ul className="token-list" aria-label="Archive tokens">
            {tokens.map((t) => (
              <li key={t.id} className="model-row">
                <span className="path">
                  <strong>{t.name}</strong> · {scopeSummary(t.scopes)} · created {formatItemDate(t.created)} · last used{" "}
                  {t.last_used ? formatItemDate(t.last_used) : "never"}
                </span>
                {confirmRevoke === t.id ? (
                  <>
                    <button type="button" className="btn-ghost" disabled={working} onClick={() => revoke(t.id)}>
                      Revoke now
                    </button>
                    <button type="button" className="btn-ghost" disabled={working} onClick={() => setConfirmRevoke(null)}>
                      Cancel
                    </button>
                  </>
                ) : (
                  <button type="button" className="btn-ghost" disabled={working} onClick={() => setConfirmRevoke(t.id)}>
                    Revoke…
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="field field-col">
        <div className="field-label">
          <span>New token</span>
          <small>name it after the script that will use it</small>
        </div>
        <div className="field-stack">
          <input
            type="text"
            value={name}
            maxLength={64}
            placeholder="e.g. clipboard to note"
            aria-label="Token name"
            onChange={(e) => setName(e.target.value)}
          />
          {nameError && <small className="endpoint-note">{nameError}</small>}
          {SCOPES.map((s) => (
            <label key={s.id} className="check-row">
              <input
                type="checkbox"
                checked={scopes.includes(s.id)}
                aria-label={`Scope: ${s.label}`}
                aria-describedby={`scope-hint-${s.id}`}
                onChange={(e) => setScopes(toggleScope(scopes, s.id, e.target.checked))}
              />
              <span>
                {s.label}
                <small id={`scope-hint-${s.id}`}> {s.hint}</small>
              </span>
            </label>
          ))}
          <div className="list-actions start">
            <button type="button" className="btn-ghost" disabled={!canCreate} onClick={create}>
              Create token
            </button>
          </div>
        </div>
      </div>
    </Card>
  );
}
