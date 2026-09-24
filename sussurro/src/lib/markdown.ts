/* A small markdown parser for companion documents (#120): the subset LLMs
   write — ATX headings h1–h6, paragraphs, bold / italic / code / links,
   bullet, numbered and task lists (nested by indentation), block quotes,
   fenced code, horizontal rules and GFM pipe tables.

   It produces a plain data tree, never HTML: the renderer (shell/Markdown.tsx)
   turns it into React elements, so nothing in a document can inject markup
   or script, and the strict CSP (#91) holds. Raw HTML in the source stays
   literal text; images render as their alt text; links are not followed. */

export type Inline =
  | { t: "text"; v: string }
  | { t: "strong"; c: Inline[] }
  | { t: "em"; c: Inline[] }
  | { t: "del"; c: Inline[] }
  | { t: "code"; v: string }
  | { t: "link"; c: Inline[]; href: string }
  | { t: "br" };

export type Align = "left" | "center" | "right" | null;

export interface ListItem {
  /** Task-list state: true `[x]`, false `[ ]`, null for a plain item. */
  checked: boolean | null;
  content: Inline[];
  children: Block[];
}

export type Block =
  | { t: "heading"; level: 1 | 2 | 3 | 4 | 5 | 6; c: Inline[] }
  | { t: "para"; c: Inline[] }
  | { t: "list"; ordered: boolean; start: number; items: ListItem[] }
  | { t: "quote"; c: Block[] }
  | { t: "code"; lang: string; v: string }
  | { t: "table"; align: Align[]; head: Inline[][]; rows: Inline[][][] }
  | { t: "hr" };

/* ---------- inline ---------- */

const ESCAPABLE = "\\`*_{}[]()#+-.!|~>";

/** Parse inline markup. Unmatched delimiters stay literal. */
export function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let text = "";
  const flush = () => {
    if (text) out.push({ t: "text", v: text });
    text = "";
  };
  let i = 0;
  while (i < src.length) {
    const ch = src[i];
    const rest = src.slice(i);
    if (ch === "\\" && i + 1 < src.length && ESCAPABLE.includes(src[i + 1])) {
      text += src[i + 1];
      i += 2;
      continue;
    }
    if (ch === "`") {
      const run = /^`+/.exec(rest)![0];
      const end = src.indexOf(run, i + run.length);
      if (end > 0) {
        flush();
        out.push({ t: "code", v: src.slice(i + run.length, end).trim() });
        i = end + run.length;
        continue;
      }
      text += run;
      i += run.length;
      continue;
    }
    // Image: keep only its alt text (img-src is locked down, #91).
    if (ch === "!" && src[i + 1] === "[") {
      const m = /^!\[([^\]]*)\]\(([^)\s]*)(?:\s+"[^"]*")?\)/.exec(rest);
      if (m) {
        text += m[1];
        i += m[0].length;
        continue;
      }
    }
    if (ch === "[") {
      const m = /^\[((?:\\.|[^\]\\])*)\]\(([^)\s]*)(?:\s+"[^"]*")?\)/.exec(rest);
      if (m) {
        flush();
        out.push({ t: "link", c: parseInline(m[1]), href: m[2] });
        i += m[0].length;
        continue;
      }
    }
    if (ch === "*" || ch === "_" || ch === "~") {
      const double = src[i + 1] === ch;
      if (ch === "~" && !double) {
        text += ch;
        i++;
        continue;
      }
      const delim = double ? ch + ch : ch;
      const inner = i + delim.length;
      // Opening: not followed by a space; `_` must not sit inside a word.
      const prev = i > 0 ? src[i - 1] : " ";
      const opens = inner < src.length && !/\s/.test(src[inner]) && !(ch === "_" && /\w/.test(prev));
      if (opens) {
        const close = findClose(src, inner, delim);
        if (close > inner) {
          flush();
          const c = parseInline(src.slice(inner, close));
          out.push(ch === "~" ? { t: "del", c } : double ? { t: "strong", c } : { t: "em", c });
          i = close + delim.length;
          continue;
        }
      }
      text += delim;
      i += delim.length;
      continue;
    }
    text += ch;
    i++;
  }
  flush();
  return out;
}

/** Index of the closing `delim` after `from` (not preceded by a space, not
 *  inside a code span, `_` not followed by a word character), or -1. */
function findClose(src: string, from: number, delim: string): number {
  let i = from;
  while (i < src.length) {
    if (src[i] === "\\") {
      i += 2;
      continue;
    }
    if (src[i] === "`") {
      const run = /^`+/.exec(src.slice(i))![0];
      const end = src.indexOf(run, i + run.length);
      i = end > 0 ? end + run.length : i + run.length;
      continue;
    }
    if (src.startsWith(delim, i) && !/\s/.test(src[i - 1])) {
      const after = src[i + delim.length] ?? " ";
      // A single `*` right before another `*` belongs to a `**`.
      if (delim.length === 1 && after === delim) {
        i += 2;
        continue;
      }
      if (delim[0] === "_" && /\w/.test(after)) {
        i++;
        continue;
      }
      return i;
    }
    i++;
  }
  return -1;
}

/** Plain text of inline nodes (for matching and labels). */
export function inlineText(nodes: Inline[]): string {
  return nodes
    .map((n) => {
      switch (n.t) {
        case "text":
        case "code":
          return n.v;
        case "br":
          return "\n";
        default:
          return inlineText(n.c);
      }
    })
    .join("");
}

/* ---------- blocks ---------- */

const HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/;
const HR = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)[^`]*$/;
const QUOTE = /^ {0,3}>\s?(.*)$/;
const ITEM = /^(\s*)([-*+]|\d{1,9}[.)])[ \t]+(.*)$/;
const EMPTY_ITEM = /^(\s*)([-*+]|\d{1,9}[.)])$/;
const TABLE_SEP = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;

function indentOf(line: string): number {
  let n = 0;
  for (const ch of line) {
    if (ch === " ") n++;
    else if (ch === "\t") n += 4;
    else break;
  }
  return n;
}

/** Split a table row on unescaped pipes, dropping the outer ones. */
export function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith("|")) s = s.slice(1);
  if (s.endsWith("|") && !s.endsWith("\\|")) s = s.slice(0, -1);
  const cells: string[] = [];
  let cur = "";
  let inCode = false;
  for (let i = 0; i < s.length; i++) {
    const ch = s[i];
    if (ch === "\\" && s[i + 1] === "|") {
      cur += "|";
      i++;
    } else if (ch === "`") {
      inCode = !inCode;
      cur += ch;
    } else if (ch === "|" && !inCode) {
      cells.push(cur.trim());
      cur = "";
    } else cur += ch;
  }
  cells.push(cur.trim());
  return cells;
}

function alignOf(cell: string): Align {
  const c = cell.trim();
  const left = c.startsWith(":");
  const right = c.endsWith(":");
  return left && right ? "center" : right ? "right" : left ? "left" : null;
}

function startsBlock(line: string, next: string | undefined): boolean {
  return (
    HEADING.test(line) ||
    HR.test(line) ||
    FENCE.test(line) ||
    QUOTE.test(line) ||
    ITEM.test(line) ||
    (line.includes("|") && next !== undefined && TABLE_SEP.test(next) && next.includes("-"))
  );
}

/** Parse a markdown document (without frontmatter) into blocks. */
export function parseMarkdown(src: string): Block[] {
  return parseLines(src.replace(/\r\n?/g, "\n").split("\n"));
}

function parseLines(lines: string[]): Block[] {
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      i++;
      continue;
    }
    const fence = FENCE.exec(line);
    if (fence) {
      const marker = fence[1];
      const body: string[] = [];
      i++;
      while (i < lines.length && !lines[i].trim().startsWith(marker)) body.push(lines[i++]);
      i++; // closing fence (or end of document)
      blocks.push({ t: "code", lang: fence[2] ?? "", v: body.join("\n") });
      continue;
    }
    const h = HEADING.exec(line);
    if (h) {
      const level = h[1].length as 1 | 2 | 3 | 4 | 5 | 6;
      blocks.push({ t: "heading", level, c: parseInline(h[2] ?? "") });
      i++;
      continue;
    }
    if (HR.test(line)) {
      blocks.push({ t: "hr" });
      i++;
      continue;
    }
    if (QUOTE.test(line)) {
      const inner: string[] = [];
      while (i < lines.length && lines[i].trim()) {
        const q = QUOTE.exec(lines[i]);
        if (q) inner.push(q[1]);
        else if (inner.length && !startsBlock(lines[i], lines[i + 1])) inner.push(lines[i]); // lazy continuation
        else break;
        i++;
      }
      blocks.push({ t: "quote", c: parseLines(inner) });
      continue;
    }
    const next = lines[i + 1];
    if (line.includes("|") && next !== undefined && TABLE_SEP.test(next) && next.includes("-")) {
      const head = splitRow(line);
      const align = splitRow(next).map(alignOf);
      const rows: Inline[][][] = [];
      i += 2;
      while (i < lines.length && lines[i].trim() && lines[i].includes("|")) {
        const cells = splitRow(lines[i]);
        rows.push(head.map((_, k) => parseInline(cells[k] ?? "")));
        i++;
      }
      blocks.push({
        t: "table",
        align: head.map((_, k) => align[k] ?? null),
        head: head.map(parseInline),
        rows,
      });
      continue;
    }
    if (ITEM.test(line) || EMPTY_ITEM.test(line)) {
      const [list, end] = parseList(lines, i);
      blocks.push(list);
      i = end;
      continue;
    }
    // Paragraph: until a blank line or the start of another block.
    const para: string[] = [line.trim()];
    i++;
    while (i < lines.length && lines[i].trim() && !startsBlock(lines[i], lines[i + 1])) {
      para.push(lines[i].trim());
      i++;
    }
    blocks.push({ t: "para", c: joinLines(para) });
  }
  return blocks;
}

/** Paragraph lines: soft breaks become spaces, a trailing `\` or two
 *  spaces a hard break. */
function joinLines(lines: string[]): Inline[] {
  const out: Inline[] = [];
  lines.forEach((l, k) => {
    const hard = k < lines.length - 1 && /(\\| {2,})$/.test(l);
    out.push(...parseInline(hard ? l.replace(/(\\| +)$/, "") : l));
    if (k < lines.length - 1) out.push(hard ? { t: "br" } : { t: "text", v: " " });
  });
  return mergeText(out);
}

function mergeText(nodes: Inline[]): Inline[] {
  const out: Inline[] = [];
  for (const n of nodes) {
    const last = out[out.length - 1];
    if (n.t === "text" && last?.t === "text") last.v += n.v;
    else out.push(n.t === "text" ? { ...n } : n);
  }
  return out;
}

/** A list starting at `start`; returns it and the index after it. Items
 *  indented deeper than the list's own markers nest. */
function parseList(lines: string[], start: number): [Block, number] {
  const first = ITEM.exec(lines[start]) ?? EMPTY_ITEM.exec(lines[start])!;
  const base = indentOf(first[1]);
  const ordered = /\d/.test(first[2]);
  const items: ListItem[] = [];
  let i = start;
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      // A blank line ends the list unless an item of this list follows.
      let j = i + 1;
      while (j < lines.length && !lines[j].trim()) j++;
      const m = j < lines.length ? ITEM.exec(lines[j]) : null;
      if (m && indentOf(m[1]) >= base && /\d/.test(m[2]) === ordered) {
        i = j;
        continue;
      }
      break;
    }
    const m = ITEM.exec(line) ?? EMPTY_ITEM.exec(line);
    const indent = indentOf(line);
    // Same level (a stray space of difference is forgiven).
    if (m && indent >= base && indent <= base + 1) {
      if (/\d/.test(m[2]) !== ordered) break;
      let text = m[3] ?? "";
      let checked: boolean | null = null;
      const task = /^\[([ xX])\]\s+(.*)$/.exec(text);
      if (task) {
        checked = task[1] !== " ";
        text = task[2];
      }
      items.push({ checked, content: parseInline(text.trim()), children: [] });
      i++;
      continue;
    }
    if (indent > base + 1 && items.length) {
      // Deeper lines belong to the last item: a nested list, or more text.
      const nested: string[] = [];
      while (i < lines.length) {
        if (!lines[i].trim()) {
          const nextLine = lines.slice(i + 1).find((l) => l.trim());
          if (!nextLine || indentOf(nextLine) <= base) break;
        } else if (indentOf(lines[i]) <= base) break;
        nested.push(lines[i]);
        i++;
      }
      const cut = Math.min(...nested.filter((l) => l.trim()).map(indentOf));
      const dedented = nested.map((l) => l.replace(new RegExp(`^[ \\t]{0,${cut}}`), ""));
      const children = parseLines(dedented);
      const item = items[items.length - 1];
      const startsWithItem = ITEM.test(dedented[0]) || EMPTY_ITEM.test(dedented[0]);
      if (!startsWithItem && !item.children.length && children[0]?.t === "para") {
        const para = children.shift() as { t: "para"; c: Inline[] };
        item.content = mergeText([...item.content, { t: "text", v: " " }, ...para.c]);
      }
      item.children.push(...children);
      continue;
    }
    if (m && indent < base) break;
    // Lazy continuation line of the last item's text.
    if (items.length && !startsBlock(line, lines[i + 1])) {
      const item = items[items.length - 1];
      item.content = mergeText([...item.content, { t: "text", v: " " }, ...parseInline(line.trim())]);
      i++;
      continue;
    }
    break;
  }
  const startNum = ordered ? parseInt(first[2], 10) : 1;
  return [{ t: "list", ordered, start: startNum, items }, i];
}

/* ---------- document helpers ---------- */

const TLDR = /^\s*(?:tl;?dr|tldr)\s*[:.\-–—]?\s*/i;

/** The document's tl;dr, when its first block is a paragraph (or a quote
 *  holding one) that opens with "tl;dr" — the convention of the Formatted
 *  document recipe. Returns the summary inline nodes (label removed) and
 *  the remaining blocks, or null. */
export function splitTldr(blocks: Block[]): { tldr: Inline[]; rest: Block[] } | null {
  const first = blocks[0];
  if (!first) return null;
  let para: Inline[] | null = null;
  if (first.t === "para") para = first.c;
  else if (first.t === "quote" && first.c.length === 1 && first.c[0].t === "para") para = first.c[0].c;
  if (!para || !TLDR.test(inlineText(para))) return null;
  return { tldr: stripLabel(para), rest: blocks.slice(1) };
}

/** Remove a leading "tl;dr:" — plain, or wrapped in bold/italic. */
function stripLabel(nodes: Inline[]): Inline[] {
  const [head, ...tail] = nodes;
  if (head?.t === "text") {
    const v = head.v.replace(TLDR, "");
    return v ? [{ t: "text", v }, ...tail] : tail;
  }
  if (head && (head.t === "strong" || head.t === "em")) {
    // `**tl;dr:** text` / `**tl;dr**: text` / `**tl;dr: the whole gist**`
    const inner = stripLabel(head.c);
    const rest = trimStart(tail);
    return inlineText(inner).trim() ? [{ ...head, c: inner }, ...rest] : rest;
  }
  return nodes;
}

function trimStart(nodes: Inline[]): Inline[] {
  const [head, ...tail] = nodes;
  if (head?.t !== "text") return nodes;
  const v = head.v.replace(/^[\s:.\-–—]+/, "");
  return v ? [{ t: "text", v }, ...tail] : tail;
}

/** Drop the trailing "Generated from the transcript…" note the app writes
 *  (the pane shows provenance itself): a final paragraph mentioning the
 *  transcript link, and the rule above it. */
export function stripProvenanceFooter(blocks: Block[]): Block[] {
  const last = blocks[blocks.length - 1];
  if (last?.t !== "para") return blocks;
  const hasLink = (nodes: Inline[]): boolean =>
    nodes.some((n) => (n.t === "link" && n.href === "transcript.md") || ("c" in n && hasLink(n.c)));
  if (!hasLink(last.c) || !/^Generated from/.test(inlineText(last.c).trim())) return blocks;
  const rest = blocks.slice(0, -1);
  return rest[rest.length - 1]?.t === "hr" ? rest.slice(0, -1) : rest;
}
