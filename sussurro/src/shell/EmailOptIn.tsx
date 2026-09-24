/** Per-run opt-in to send the participants' emails with a recipe or a
 *  question (#143). Participants go to the model by name only unless this
 *  is ticked; it is never remembered (each run starts unticked), and on
 *  an external profile the confirmation is bound to it. Hidden when the
 *  item has no participant email. */
export function EmailOptIn({
  count,
  checked,
  onChange,
  disabled = false,
  className = "",
}: {
  /** Participants with an email. */
  count: number;
  checked: boolean;
  onChange: (on: boolean) => void;
  disabled?: boolean;
  className?: string;
}) {
  if (!count) return null;
  return (
    <label className={`check-row email-opt-in ${className}`.trim()}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
      <span>
        Include participant emails{" "}
        <span className="sh-muted">
          ({count}) — this run only; otherwise the model gets names only
        </span>
      </span>
    </label>
  );
}
