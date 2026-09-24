import type { Ctl } from "../hooks/useAppController";
import {
  bundledNote,
  bundledProblem,
  cleanupProfile,
  formatGb,
  offerBundled,
  profileHost,
} from "../lib/llmProfiles";

/** "Use the bundled model (downloads ~2.2 GB)" — the one control that
 *  moves cleanup onto the built-in profile (#118), always a click. */
export function UseBundledButton({ ctl, label }: { ctl: Ctl; label?: string }) {
  const { bundledLlm, bundledBusy } = ctl;
  const size = bundledLlm && !bundledLlm.downloaded ? ` (downloads ~${formatGb(bundledLlm.download_bytes)})` : "";
  return (
    <button type="button" className="btn-ghost" disabled={bundledBusy} onClick={() => ctl.pickBundledForCleanup()}>
      {bundledBusy && <span className="btn-spinner" aria-hidden="true" />}
      {bundledBusy ? "Preparing the bundled model…" : `${label ?? "Use the bundled model"}${size}`}
    </button>
  );
}

/** Onboarding (#118): the cleanup profile's server can't be reached and the
 *  build can run the bundled model — offer it. `inline` = a setup-banner
 *  list item; otherwise a note under the cleanup profile field. */
export function BundledOffer({ ctl, inline = false }: { ctl: Ctl; inline?: boolean }) {
  const { settings, bundledLlm, ollamaStatus } = ctl;
  if (!offerBundled(settings, bundledLlm, ollamaStatus)) return null;
  const profile = cleanupProfile(settings);
  const where = profile ? `“${profile.name}” (${profileHost(profile)})` : "the cleanup server";
  if (inline) {
    return (
      <li>
        <span className="setup-bad">✗</span> No cleanup server reachable — or let Sussurro run a small model
        itself, nothing to install.
        <UseBundledButton ctl={ctl} />
      </li>
    );
  }
  const text = `No cleanup server is reachable at ${where}. Sussurro can run a small model itself instead: no Ollama or other server to install, and nothing leaves this machine.`;
  return (
    <div className="field field-col bundled-offer" role="note">
      <small className="endpoint-note">{text}</small>
      <div className="row-gap">
        <UseBundledButton ctl={ctl} />
      </div>
    </div>
  );
}

/** Cleanup is on the bundled profile but can't run yet: say why, and offer
 *  the download when that is the reason. */
export function BundledProblem({ ctl, inline = false }: { ctl: Ctl; inline?: boolean }) {
  const { settings, bundledLlm, bundledBusy } = ctl;
  const problem = bundledProblem(settings, bundledLlm);
  if (!problem) return null;
  const button = bundledLlm?.available && !bundledLlm.downloaded && (
    <button type="button" className="btn-ghost" disabled={bundledBusy} onClick={() => ctl.downloadBundledModel()}>
      {bundledBusy && <span className="btn-spinner" aria-hidden="true" />}
      {bundledBusy ? "Downloading…" : `Download (${formatGb(bundledLlm.download_bytes)})`}
    </button>
  );
  if (inline) {
    return (
      <li>
        <span className="setup-bad">✗</span> {problem}
        {button}
      </li>
    );
  }
  return (
    <div className="field field-col" role="note">
      <small className="endpoint-note">⚠ {problem}</small>
      {button && <div className="row-gap">{button}</div>}
    </div>
  );
}

/** The built-in profile in the profile list (Recipes): nothing to edit —
 *  what it is, whether it's ready, download / use for cleanup. */
export function BundledProfilePanel({ ctl, onDone }: { ctl: Ctl; onDone: () => void }) {
  const { settings, bundledLlm, bundledBusy } = ctl;
  const inUse = cleanupProfile(settings)?.bundled === true;
  const state = !bundledLlm
    ? "Checking…"
    : !bundledLlm.available
      ? "Not available in this build: it has no bundled llama-server."
      : !bundledLlm.downloaded
        ? `Not downloaded yet (${formatGb(bundledLlm.download_bytes)}).`
        : bundledLlm.running
          ? "Downloaded, running now."
          : "Downloaded. Starts on first use and stops after 15 minutes idle.";
  return (
    <div className="prof-editor" role="group" aria-label="Local (bundled)">
      <p className="card-hint bundled-note">{bundledNote(bundledLlm)}</p>
      <p className="card-hint" role="status">
        <strong>{bundledLlm?.model ?? "Bundled model"}</strong> · {state}
      </p>
      <p className="card-hint">
        Built in: its server, model and settings are fixed, and it can't be deleted. Pick it for cleanup here or in
        Settings → Cleanup, or for a recipe run.
      </p>
      <div className="row-gap prof-actions">
        {bundledLlm?.available && !bundledLlm.downloaded && (
          <button type="button" className="btn-ghost" disabled={bundledBusy} onClick={() => ctl.downloadBundledModel()}>
            {bundledBusy && <span className="btn-spinner" aria-hidden="true" />}
            {bundledBusy ? "Downloading…" : "Download"}
          </button>
        )}
        {!inUse && bundledLlm?.available && <UseBundledButton ctl={ctl} label="Use for cleanup" />}
        <button type="button" className="btn-ghost push" onClick={onDone}>
          Close
        </button>
      </div>
    </div>
  );
}
