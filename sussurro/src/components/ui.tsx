import { useState, type ReactNode } from "react";
import { isLocalEndpoint, parseEndpoint } from "../lib/endpoint";

/* ---------- Info tooltip ---------- */

export function Tip({ text }: { text: string }) {
  return (
    <span className="tip" tabIndex={0} role="note" aria-label={text}>
      ?
      <span className="tip-bubble">{text}</span>
    </span>
  );
}

/* ---------- Collapsible section ---------- */

/** A settings card. In the classic window it is an accordion whose open state
 *  is remembered per card; the workspace shows one card at a time, always open
 *  (`collapsible={false}`), with the same content. */
export function CollapsibleCard({
  storageKey,
  title,
  className = "card",
  headerExtra,
  defaultOpen = false,
  collapsible = true,
  children,
}: {
  storageKey: string;
  title: ReactNode;
  className?: string;
  headerExtra?: ReactNode;
  defaultOpen?: boolean;
  collapsible?: boolean;
  children: ReactNode;
}) {
  // First run: only the cards marked defaultOpen are expanded. Afterwards the
  // user's own open/closed choice (localStorage) always wins.
  const [open, setOpen] = useState(() => {
    const stored = localStorage.getItem(storageKey);
    return stored === null ? defaultOpen : stored === "1";
  });
  if (!collapsible) {
    return (
      <section className={`static-card ${className}`}>
        <header className="static-card-head">
          <h2>{title}</h2>
          {headerExtra && <span className="summary-right">{headerExtra}</span>}
        </header>
        {children}
      </section>
    );
  }
  return (
    <details
      className={`collapsible ${className}`}
      open={open}
      onToggle={(e) => {
        const o = (e.target as HTMLDetailsElement).open;
        setOpen(o);
        localStorage.setItem(storageKey, o ? "1" : "0");
      }}
    >
      <summary>
        <h2>{title}</h2>
        <span className="summary-right">
          {headerExtra}
          <span className="chevron" aria-hidden="true">▾</span>
        </span>
      </summary>
      {children}
    </details>
  );
}

/** Visually de-emphasized group for rarely-touched settings. */
export function AdvancedGroup({ children }: { children: ReactNode }) {
  return (
    <div className="advanced-group">
      <span className="advanced-label">Advanced</span>
      {children}
    </div>
  );
}

/** Privacy note under the cleanup Server field when the endpoint is remote. */
export function EndpointNote({ url }: { url: string }) {
  const e = parseEndpoint(url);
  if (!e || isLocalEndpoint(url)) return null;
  return (
    <small className="endpoint-note">
      ⚠ Your transcripts will be sent to {e.host} over {e.secure ? "https" : "http"}
    </small>
  );
}

/** An on/off switch styled like the rest of the settings. */
export function Switch({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** Accessible name when no visible label is associated. */
  label?: string;
}) {
  return (
    <label className="switch">
      <input
        type="checkbox"
        checked={checked}
        aria-label={label}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="slider" />
    </label>
  );
}
