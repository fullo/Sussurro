import { useCallback, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { obtainConsent, sizeLabel, type ConsentRequest } from "../lib/privacy";
import type { ExternalRunPreview, LlmProfile } from "../lib/types";

/** The per-run confirmation for an external LLM profile (#122): which
 *  document goes to which host, how much of it, with which model. Asked
 *  every time; Cancel is the default. */
export function ConsentDialog({
  preview,
  onAnswer,
}: {
  preview: ExternalRunPreview;
  onAnswer: (ok: boolean) => void;
}) {
  const task = preview.question ? `Question: “${preview.question}”` : `Recipe: ${preview.recipe_name}`;
  return (
    <div className="modal-backdrop" role="presentation" onClick={() => onAnswer(false)}>
      <div
        className="modal confirm-modal consent-modal"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="consent-title"
        aria-describedby="consent-desc"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onAnswer(false);
          }
        }}
      >
        <h2 id="consent-title">Send this to {preview.host}?</h2>
        <p id="consent-desc" className="sh-muted">
          <b>{preview.profile_name}</b> is an external profile: the text of this item leaves your computer and is
          processed on that server, under its provider's terms.
        </p>
        <dl className="consent-facts">
          <dt>Document</dt>
          <dd>{preview.item_title || "Untitled"}</dd>
          <dt>Task</dt>
          <dd>{task}</dd>
          <dt>Size</dt>
          <dd>{sizeLabel(preview.chars, preview.approx_tokens)}</dd>
          <dt>Server</dt>
          <dd>
            <span className="mono">{preview.host}</span>
            {preview.base_url && preview.base_url !== preview.host && (
              <small className="sh-muted"> · {preview.base_url}</small>
            )}
          </dd>
          <dt>Model</dt>
          <dd>{preview.model}</dd>
        </dl>
        <p className="sh-note">
          Sussurro asks every time and marks the item as sent externally in the Library. Only this run is
          allowed; nothing else is sent.
        </p>
        <div className="row-gap end">
          <button type="button" className="btn-ghost sh-btn" autoFocus onClick={() => onAnswer(false)}>
            Cancel
          </button>
          <button type="button" className="btn-dark sh-btn" onClick={() => onAnswer(true)}>
            Send to {preview.host}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Ask for the per-run confirmation where a run starts. `consentFor`
 *  resolves to `{ consent }` to start the run with (null on a local
 *  profile), or null when the user cancelled; `dialog` must be rendered. */
export function useExternalConsent() {
  const [pending, setPending] = useState<ExternalRunPreview | null>(null);
  const answer = useRef<((ok: boolean) => void) | null>(null);

  const confirm = useCallback(
    (preview: ExternalRunPreview) =>
      new Promise<boolean>((resolve) => {
        answer.current = resolve;
        setPending(preview);
      }),
    [],
  );

  const consentFor = useCallback(
    (profile: LlmProfile, req: ConsentRequest) =>
      obtainConsent((cmd, args) => invoke(cmd, args), confirm, profile, req),
    [confirm],
  );

  const dialog = pending ? (
    <ConsentDialog
      preview={pending}
      onAnswer={(ok) => {
        setPending(null);
        answer.current?.(ok);
        answer.current = null;
      }}
    />
  ) : null;

  return { consentFor, dialog, asking: pending !== null };
}
