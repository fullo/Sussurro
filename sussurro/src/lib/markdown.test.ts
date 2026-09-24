import { describe, expect, it } from "vitest";
import { inlineText, parseInline, parseMarkdown, splitRow, splitTldr, stripProvenanceFooter, type Block } from "./markdown";

describe("parseInline", () => {
  it("parses bold, italic, code, strike and links", () => {
    expect(parseInline("a **b** *c* `d` ~~e~~ [f](http://x)")).toEqual([
      { t: "text", v: "a " },
      { t: "strong", c: [{ t: "text", v: "b" }] },
      { t: "text", v: " " },
      { t: "em", c: [{ t: "text", v: "c" }] },
      { t: "text", v: " " },
      { t: "code", v: "d" },
      { t: "text", v: " " },
      { t: "del", c: [{ t: "text", v: "e" }] },
      { t: "text", v: " " },
      { t: "link", c: [{ t: "text", v: "f" }], href: "http://x" },
    ]);
  });

  it("nests emphasis inside bold", () => {
    expect(parseInline("**a *b* c**")).toEqual([
      { t: "strong", c: [{ t: "text", v: "a " }, { t: "em", c: [{ t: "text", v: "b" }] }, { t: "text", v: " c" }] },
    ]);
  });

  it("leaves unmatched delimiters, snake_case and 2 * 3 literal", () => {
    expect(inlineText(parseInline("2 * 3 and snake_case_name and **open"))).toBe("2 * 3 and snake_case_name and **open");
    expect(parseInline("snake_case_name")).toEqual([{ t: "text", v: "snake_case_name" }]);
  });

  it("keeps raw HTML as text and images as their alt text", () => {
    expect(parseInline("<script>alert(1)</script>")).toEqual([{ t: "text", v: "<script>alert(1)</script>" }]);
    expect(parseInline("![a chart](http://evil/x.png)")).toEqual([{ t: "text", v: "a chart" }]);
  });

  it("honours escapes and code spans", () => {
    expect(parseInline("\\*not em\\*")).toEqual([{ t: "text", v: "*not em*" }]);
    expect(parseInline("`a **b** c`")).toEqual([{ t: "code", v: "a **b** c" }]);
  });
});

describe("parseMarkdown", () => {
  it("parses headings h1–h6 and paragraphs with soft breaks", () => {
    const b = parseMarkdown("# One\n## Two\n###### Six ##\n\nline one\nline two\n\n#nospace");
    expect(b.map((x) => (x.t === "heading" ? x.level : x.t))).toEqual([1, 2, 6, "para", "para"]);
    expect(b[2]).toEqual({ t: "heading", level: 6, c: [{ t: "text", v: "Six" }] });
    expect(b[3]).toEqual({ t: "para", c: [{ t: "text", v: "line one line two" }] });
  });

  it("parses GFM tables with alignment and escaped pipes", () => {
    const b = parseMarkdown("| Idea | Effort | Release |\n|:---|:---:|---:|\n| Docs | **Low** | 0.7 |\n| a \\| b | x |\n");
    expect(b).toHaveLength(1);
    const t = b[0] as Extract<Block, { t: "table" }>;
    expect(t.t).toBe("table");
    expect(t.align).toEqual(["left", "center", "right"]);
    expect(t.head.map(inlineText)).toEqual(["Idea", "Effort", "Release"]);
    expect(t.rows.map((r) => r.map(inlineText))).toEqual([
      ["Docs", "Low", "0.7"],
      ["a | b", "x", ""],
    ]);
  });

  it("parses task lists, numbered lists and nesting", () => {
    const b = parseMarkdown("- [ ] Send the file — Anna\n- [x] Book the room\n  - sub point\n    continued\n\n3. three\n4. four");
    expect(b).toHaveLength(2);
    const tasks = b[0] as Extract<Block, { t: "list" }>;
    expect(tasks.ordered).toBe(false);
    expect(tasks.items.map((i) => i.checked)).toEqual([false, true]);
    expect(inlineText(tasks.items[0].content)).toBe("Send the file — Anna");
    const sub = tasks.items[1].children[0] as Extract<Block, { t: "list" }>;
    expect(sub.t).toBe("list");
    expect(inlineText(sub.items[0].content)).toBe("sub point continued");
    const num = b[1] as Extract<Block, { t: "list" }>;
    expect(num.ordered && num.start).toBe(3);
    expect(num.items).toHaveLength(2);
  });

  it("keeps a list together across blank lines between its items", () => {
    const b = parseMarkdown("- a\n\n- b\n\nafter");
    expect(b.map((x) => x.t)).toEqual(["list", "para"]);
    expect((b[0] as Extract<Block, { t: "list" }>).items).toHaveLength(2);
  });

  it("parses quotes, fences and rules", () => {
    const b = parseMarkdown("> quoted\n> more\n\n```js\nlet a = '<b>';\n```\n\n---\n***");
    expect(b[0]).toEqual({ t: "quote", c: [{ t: "para", c: [{ t: "text", v: "quoted more" }] }] });
    expect(b[1]).toEqual({ t: "code", lang: "js", v: "let a = '<b>';" });
    expect(b.slice(2)).toEqual([{ t: "hr" }, { t: "hr" }]);
  });

  it("handles CRLF and an unclosed fence", () => {
    expect(parseMarkdown("# A\r\ntext\r\n")[1]).toEqual({ t: "para", c: [{ t: "text", v: "text" }] });
    expect(parseMarkdown("```\ncode")).toEqual([{ t: "code", lang: "", v: "code" }]);
  });

  it("splits table rows without breaking code spans", () => {
    expect(splitRow("| `a|b` | c |")).toEqual(["`a|b`", "c"]);
  });
});

describe("splitTldr", () => {
  it.each([
    "**tl;dr:** Three ideas.",
    "**TL;DR**: Three ideas.",
    "tl;dr — Three ideas.",
    "> **tl;dr:** Three ideas.",
  ])("finds the tl;dr in %s", (src) => {
    const r = splitTldr(parseMarkdown(`${src}\n\n# Title`));
    expect(r).not.toBeNull();
    expect(inlineText(r!.tldr)).toBe("Three ideas.");
    expect(r!.rest).toEqual([{ t: "heading", level: 1, c: [{ t: "text", v: "Title" }] }]);
  });

  it("keeps a whole-bold tl;dr bold", () => {
    const r = splitTldr(parseMarkdown("**TL;DR: all of it**"));
    expect(r!.tldr).toEqual([{ t: "strong", c: [{ t: "text", v: "all of it" }] }]);
  });

  it("is null without one", () => {
    expect(splitTldr(parseMarkdown("# Title\n\ntl;dr later"))).toBeNull();
    expect(splitTldr([])).toBeNull();
  });
});

describe("stripProvenanceFooter", () => {
  it("drops the app's closing note and its rule", () => {
    const b = parseMarkdown("# Doc\n\n---\n\n*Generated from [the transcript](transcript.md) by Summary / Local / m.*\n");
    expect(stripProvenanceFooter(b)).toEqual([{ t: "heading", level: 1, c: [{ t: "text", v: "Doc" }] }]);
  });

  it("keeps a user's own last paragraph", () => {
    const b = parseMarkdown("# Doc\n\nSee [the transcript](transcript.md).");
    expect(stripProvenanceFooter(b)).toBe(b);
  });
});
