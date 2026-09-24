import { useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

/* ---------- About dialog ---------- */

interface LicensePackage {
  name: string;
  version: string;
  /** The single license we use it under (dual "A OR B" resolved to one). */
  license: string;
  /** Original SPDX expression, only when it differed from `license`. */
  spdx: string;
  repository: string;
  /** `model`: a model downloaded on first use (attribution, #130). */
  ecosystem: "rust" | "npm" | "model";
  textId: number;
}
interface LicensesData {
  packages: LicensePackage[];
  texts: string[];
}

export function AboutDialog({
  version,
  onClose,
}: {
  version: string;
  onClose: () => void;
}) {
  const [data, setData] = useState<LicensesData | null>(null);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<string | null>(null);

  // Load the (large) license list lazily — it's a static asset, not bundled.
  useEffect(() => {
    fetch("/licenses.json")
      .then((r) => (r.ok ? r.json() : Promise.reject(r.status)))
      .then(setData)
      .catch((e) => setError(`Couldn't load licenses (${e})`));
  }, []);

  // Close on Escape.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const q = query.trim().toLowerCase();
  const packages = (data?.packages ?? []).filter(
    (p) =>
      !q ||
      p.name.toLowerCase().includes(q) ||
      p.license.toLowerCase().includes(q),
  );
  const rustCount = data?.packages.filter((p) => p.ecosystem === "rust").length ?? 0;
  const modelCount = data?.packages.filter((p) => p.ecosystem === "model").length ?? 0;
  const npmCount = (data?.packages.length ?? 0) - rustCount - modelCount;

  return (
    <div
      className="modal-backdrop"
      onClick={onClose}
      role="presentation"
    >
      <div
        className="modal about-modal"
        role="dialog"
        aria-modal="true"
        aria-label="About Sussurro"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <div className="brand">
            <span className="daruma idle" aria-hidden="true" />
            <h2>Sussurro {version}</h2>
          </div>
          <button
            type="button"
            className="btn-ghost modal-close"
            onClick={onClose}
            aria-label="Close"
          >
            ×
          </button>
        </div>

        <p className="about-tagline">
          Fully-local voice dictation. Your voice never leaves this machine.
        </p>
        <p className="about-credit">
          Made with a daruma's patience by{" "}
          <a
            href="https://darumahq.it"
            onClick={(e) => {
              e.preventDefault();
              openUrl("https://darumahq.it/");
            }}
          >
            DarumaHQ.it
          </a>
          {" · "}
          <a
            href="https://github.com/fullo/Sussurro"
            onClick={(e) => {
              e.preventDefault();
              openUrl("https://github.com/fullo/Sussurro");
            }}
          >
            Source on GitHub
          </a>
        </p>

        <div className="about-licenses">
          <div className="about-licenses-head">
            <h3>Third-party licenses</h3>
            {data && (
              <span className="license-count">
                {rustCount} Rust · {npmCount} npm
                {modelCount > 0 && ` · ${modelCount} model${modelCount === 1 ? "" : "s"}`}
              </span>
            )}
          </div>

          {error && <p className="busy" role="alert">{error}</p>}
          {!data && !error && <p className="muted">Loading licenses…</p>}

          {data && (
            <>
              <input
                type="search"
                className="license-search"
                placeholder="Filter by name or license…"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
              <ul className="license-list">
                {packages.map((p) => {
                  const key = `${p.ecosystem}:${p.name}@${p.version}`;
                  const isOpen = expanded === key;
                  const text = p.textId >= 0 ? data.texts[p.textId] : "";
                  return (
                    <li key={key} className="license-item">
                      <button
                        type="button"
                        className="license-row"
                        aria-expanded={isOpen}
                        onClick={() => setExpanded(isOpen ? null : key)}
                      >
                        <span className="license-name">
                          {p.name}
                          <span className="license-ver"> {p.version}</span>
                        </span>
                        <span className="license-id">{p.license}</span>
                      </button>
                      {isOpen && (
                        <div className="license-detail">
                          {p.spdx && (
                            <p className="license-spdx">
                              Offered under <code>{p.spdx}</code> — used here
                              under <code>{p.license}</code>.
                            </p>
                          )}
                          {p.repository && (
                            <a
                              href={p.repository}
                              onClick={(e) => {
                                e.preventDefault();
                                openUrl(p.repository);
                              }}
                            >
                              {p.repository}
                            </a>
                          )}
                          <pre>{text || "No license text bundled — see the SPDX identifier above."}</pre>
                        </div>
                      )}
                    </li>
                  );
                })}
                {packages.length === 0 && (
                  <li className="muted">No packages match “{query}”.</li>
                )}
              </ul>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
