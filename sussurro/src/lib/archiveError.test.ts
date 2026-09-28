import { describe, expect, it } from "vitest";
import { errorText, parseArchiveUnreadable, unreadableCopy, type ArchiveUnreadable } from "./archiveError";

const coded = (over: Partial<ArchiveUnreadable> = {}) =>
  JSON.stringify({
    code: "archive_unreadable",
    path: "/Users/anna/Documents/Sussurro",
    kind: "permission_denied",
    permission: true,
    reason: "Operation not permitted (os error 1)",
    message: "the archive folder /Users/anna/Documents/Sussurro can't be read: Operation not permitted (os error 1)",
    ...over,
  });

const MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)";
const LINUX = "Mozilla/5.0 (X11; Linux x86_64)";

describe("parseArchiveUnreadable", () => {
  it("recognises the coded error from invoke", () => {
    const e = parseArchiveUnreadable(coded());
    expect(e?.path).toBe("/Users/anna/Documents/Sussurro");
    expect(e?.permission).toBe(true);
    expect(e?.kind).toBe("permission_denied");
    expect(parseArchiveUnreadable(new Error(coded()))?.kind).toBe("permission_denied");
  });

  it("ignores every other error", () => {
    expect(parseArchiveUnreadable("no archive item 'x'")).toBeNull();
    expect(parseArchiveUnreadable('{"code":"internal"}')).toBeNull();
    expect(parseArchiveUnreadable("{not json")).toBeNull();
    expect(parseArchiveUnreadable(null)).toBeNull();
    expect(parseArchiveUnreadable(42)).toBeNull();
  });

  it("falls back on unknown kinds", () => {
    expect(parseArchiveUnreadable(coded({ kind: "weird" as never }))?.kind).toBe("other");
  });
});

describe("errorText", () => {
  it("shows the message of a coded error, anything else as is", () => {
    expect(errorText(coded())).toContain("can't be read");
    expect(errorText(coded())).not.toContain("{");
    expect(errorText("disk full")).toBe("disk full");
  });
});

describe("unreadableCopy", () => {
  const err = parseArchiveUnreadable(coded())!;

  it("points macOS permission errors to Privacy & Security", () => {
    const c = unreadableCopy(err, MAC);
    expect(c.title).toBe("Can't open the archive folder");
    expect(c.explanation).toMatch(/isn't allowed/);
    expect(c.hint).toContain("System Settings → Privacy & Security → Files and Folders");
    expect(c.hint).toContain("Full Disk Access");
    expect(c.osReason).toBe("Operation not permitted (os error 1)");
    expect(c.reassurance).toMatch(/Nothing was deleted/);
  });

  it("gives no macOS hint elsewhere", () => {
    expect(unreadableCopy(err, LINUX).hint).not.toContain("System Settings");
  });

  it("explains a missing folder", () => {
    const c = unreadableCopy(
      parseArchiveUnreadable(coded({ kind: "missing", permission: false, reason: "No such file or directory (os error 2)" }))!,
      MAC,
    );
    expect(c.explanation).toMatch(/moved or renamed/);
    expect(c.hint).not.toContain("Privacy");
  });

  it("explains a file in place of the folder", () => {
    const c = unreadableCopy(parseArchiveUnreadable(coded({ kind: "not_a_directory", permission: false }))!, MAC);
    expect(c.explanation).toMatch(/isn't a folder/);
    expect(c.hint).toBeNull();
  });
});
