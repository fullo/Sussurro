import type { ReactNode } from "react";
import { parseEndpoint } from "../lib/endpoint";

/* ---------- Info tooltip ---------- */

export function Tip({ text }: { text: string }) {
  return (
    <span className="tip" tabIndex={0} role="note" aria-label={text}>
      ?
      <span className="tip-bubble">{text}</span>
    </span>
  );
}

/* ---------- Card ---------- */

/** A titled card: a settings section, a Models or Recipes panel. */
export function Card({
  title,
  className = "card",
  headerExtra,
  children,
}: {
  title: ReactNode;
  className?: string;
  headerExtra?: ReactNode;
  children: ReactNode;
}) {
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

/** Visually de-emphasized group for rarely-touched settings. */
export function AdvancedGroup({ children }: { children: ReactNode }) {
  return (
    <div className="advanced-group">
      <span className="advanced-label">Advanced</span>
      {children}
    </div>
  );
}

/** Privacy note for an LLM profile marked external (#92, #119). The flag,
 *  not the URL, decides: it is inferred from the URL and can be set by hand. */
export function EndpointNote({ url, external }: { url: string; external: boolean }) {
  if (!external) return null;
  const e = parseEndpoint(url);
  return (
    <small className="endpoint-note">
      {e
        ? `⚠ Your transcripts will be sent to ${e.host} over ${e.secure ? "https" : "http"}`
        : "⚠ This profile is marked external: your transcripts leave this machine"}
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
