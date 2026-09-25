import { describe, expect, it } from "vitest";
import { isReactInnerHtml, unacceptedFindings, type Finding, type LintResult } from "./lint-policy";

// The two flagged sites in the 0.10.0 page chunk, verbatim (line 9 of
// assets/page-*.js): React DOM's setProp / setPropOnCustomElement.
const REACT = 'throw Error(v(60));n?.__html!==u&&(l.innerHTML=u)}}break;case"multiple":';
const col = (text: string, needle: string) => text.indexOf(needle) + 1;

const result = (warnings: Finding[], errors: Finding[] = []): LintResult => ({ errors, warnings, notices: [] });
const reactWarning = (column = col(REACT, "l.innerHTML")): Finding => ({
  code: "UNSAFE_VAR_ASSIGNMENT",
  file: "assets/page-CfYTpLUN.js",
  line: 9,
  column,
});
const android: Finding = { code: "KEY_FIREFOX_ANDROID_UNSUPPORTED_BY_MIN_VERSION", file: "manifest.json" };

describe("isReactInnerHtml", () => {
  it("recognises React DOM's dangerouslySetInnerHTML write at the flagged column", () => {
    expect(isReactInnerHtml(REACT, col(REACT, "l.innerHTML"))).toBe(true);
    // Unminified-target spelling too.
    const old = "(n==null?void 0:n.__html)!==u&&(l.innerHTML=u)";
    expect(isReactInnerHtml(old, col(old, "l.innerHTML"))).toBe(true);
  });

  it("rejects any other innerHTML assignment, or a column elsewhere", () => {
    const ours = "el.innerHTML=text;";
    expect(isReactInnerHtml(ours, col(ours, "el.innerHTML"))).toBe(false);
    expect(isReactInnerHtml(REACT, 1)).toBe(false);
    expect(isReactInnerHtml(REACT + 'x.innerHTML=y', col(REACT + "x.innerHTML=y", "x.innerHTML"))).toBe(false);
  });
});

describe("unacceptedFindings", () => {
  const lineOf = () => REACT;

  it("passes the documented set: Android min version + React DOM's two innerHTML sites", () => {
    expect(unacceptedFindings(result([android, reactWarning(), reactWarning()]), lineOf)).toEqual([]);
  });

  it("fails on any error", () => {
    expect(unacceptedFindings(result([], [{ code: "JSON_INVALID", file: "manifest.json" }]), lineOf)).toEqual([
      "error: JSON_INVALID manifest.json",
    ]);
  });

  it("fails on the desktop min-version warning (fixed by strict_min_version 140) and on unknown warnings", () => {
    const desktop = { code: "KEY_FIREFOX_UNSUPPORTED_BY_MIN_VERSION", file: "manifest.json" };
    expect(unacceptedFindings(result([desktop]), lineOf)).toHaveLength(1);
    expect(unacceptedFindings(result([{ code: "NO_DOCUMENT_WRITE", file: "assets/page-x.js", line: 1, column: 1 }]), lineOf)).toHaveLength(1);
  });

  it("fails on an innerHTML write that is not React's, or outside the page chunk", () => {
    expect(unacceptedFindings(result([reactWarning(1)]), lineOf)).toHaveLength(1);
    expect(unacceptedFindings(result([{ ...reactWarning(), file: "sidepanel-abc.js" }]), lineOf)).toHaveLength(1);
    expect(unacceptedFindings(result([reactWarning()]), () => "el.innerHTML=text")).toHaveLength(1);
  });

  it("fails if more than React DOM's two sites show up", () => {
    const bad = unacceptedFindings(result([reactWarning(), reactWarning(), reactWarning()]), lineOf);
    expect(bad).toEqual(["warning: 3 React DOM innerHTML sites, expected at most 2"]);
  });
});
