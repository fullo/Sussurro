import { Fragment, type ReactNode } from "react";
import { parseMarkdown, splitTldr, stripProvenanceFooter, type Block, type Inline } from "../lib/markdown";

/* Companion documents rendered as React elements from the parsed tree
   (lib/markdown.ts): no HTML string ever reaches the DOM, so a document
   can't inject markup or script. Links are shown, not followed — the
   webview must not navigate away — with the target in a tooltip. */

function inline(nodes: Inline[]): ReactNode[] {
  return nodes.map((n, k) => {
    switch (n.t) {
      case "text":
        return <Fragment key={k}>{n.v}</Fragment>;
      case "strong":
        return <strong key={k}>{inline(n.c)}</strong>;
      case "em":
        return <em key={k}>{inline(n.c)}</em>;
      case "del":
        return <del key={k}>{inline(n.c)}</del>;
      case "code":
        return <code key={k}>{n.v}</code>;
      case "br":
        return <br key={k} />;
      case "link":
        return (
          <span key={k} className="md-link" title={n.href}>
            {inline(n.c)}
          </span>
        );
    }
  });
}

function block(b: Block, k: number): ReactNode {
  switch (b.t) {
    case "heading": {
      const H = `h${b.level}` as const;
      return <H key={k}>{inline(b.c)}</H>;
    }
    case "para":
      return <p key={k}>{inline(b.c)}</p>;
    case "hr":
      return <hr key={k} />;
    case "code":
      return (
        <pre key={k} className="md-code">
          <code>{b.v}</code>
        </pre>
      );
    case "quote":
      return <blockquote key={k}>{b.c.map(block)}</blockquote>;
    case "list": {
      const items = b.items.map((it, j) => (
        <li key={j} className={it.checked === null ? undefined : "md-task"}>
          {it.checked !== null && (
            <span className={`md-check${it.checked ? " on" : ""}`} role="img" aria-label={it.checked ? "done" : "to do"}>
              {it.checked ? "☑" : "☐"}
            </span>
          )}
          {inline(it.content)}
          {it.children.map(block)}
        </li>
      ));
      return b.ordered ? (
        <ol key={k} start={b.start}>{items}</ol>
      ) : (
        <ul key={k} className={b.items.some((i) => i.checked !== null) ? "md-tasks" : undefined}>{items}</ul>
      );
    }
    case "table":
      return (
        <div key={k} className="md-table-wrap">
          <table>
            <thead>
              <tr>
                {b.head.map((c, j) => (
                  <th key={j} style={b.align[j] ? { textAlign: b.align[j]! } : undefined}>{inline(c)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {b.rows.map((r, i) => (
                <tr key={i}>
                  {r.map((c, j) => (
                    <td key={j} style={b.align[j] ? { textAlign: b.align[j]! } : undefined}>{inline(c)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
  }
}

/** A markdown document; a leading tl;dr is shown as a highlighted box. */
export function Markdown({ source, label }: { source: string; label: string }) {
  const blocks = stripProvenanceFooter(parseMarkdown(source));
  const split = splitTldr(blocks);
  return (
    <div className="md" aria-label={label}>
      {split && (
        <div className="md-tldr">
          <b>tl;dr</b>
          {inline(split.tldr)}
        </div>
      )}
      {(split ? split.rest : blocks).map(block)}
    </div>
  );
}
