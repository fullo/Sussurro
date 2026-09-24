import { describe, expect, it } from "vitest";
import {
  baseName,
  fileManagerName,
  fmtCount,
  formatClock,
  formatDurationLabel,
  formatItemDate,
  formatTimestamp,
  parseDuration,
  progressPercent,
} from "./format";

describe("fmtCount", () => {
  it("groups thousands and abbreviates from 10k", () => {
    expect(fmtCount(12)).toBe("12");
    expect(fmtCount(1234)).toBe("1,234");
    expect(fmtCount(15200)).toBe("15.2k");
  });
});

describe("formatTimestamp", () => {
  it("matches the backend's HH:MM:SS", () => {
    expect(formatTimestamp(0)).toBe("00:00:00");
    expect(formatTimestamp(4_999)).toBe("00:00:04");
    expect(formatTimestamp(14 * 60_000 + 29_000)).toBe("00:14:29");
    expect(formatTimestamp(3_723_000)).toBe("01:02:03");
    expect(formatTimestamp(-5)).toBe("00:00:00");
  });
});

describe("formatClock", () => {
  it("shows MM:SS, and hours only past one hour", () => {
    expect(formatClock(0)).toBe("00:00");
    expect(formatClock(75.9)).toBe("01:15");
    expect(formatClock(3600 + 61)).toBe("1:01:01");
  });
});

describe("parseDuration / formatDurationLabel", () => {
  it("parses frontmatter durations", () => {
    expect(parseDuration("00:42:10")).toBe(2530);
    expect(parseDuration("03:12")).toBe(192);
    expect(parseDuration("45")).toBe(45);
    expect(parseDuration("")).toBeNull();
    expect(parseDuration(undefined)).toBeNull();
    expect(parseDuration("1h")).toBeNull();
    expect(parseDuration("1:2:3:4")).toBeNull();
  });

  it("labels lengths like the mock", () => {
    expect(formatDurationLabel(null)).toBe("");
    expect(formatDurationLabel(45)).toBe("45 s");
    expect(formatDurationLabel(192)).toBe("3 min");
    expect(formatDurationLabel(48 * 60)).toBe("48 min");
    expect(formatDurationLabel(72 * 60)).toBe("1 h 12");
    expect(formatDurationLabel(2 * 3600)).toBe("2 h");
  });
});

describe("formatItemDate", () => {
  const now = new Date(2026, 8, 24, 15, 0); // 24 Sep 2026, local time

  it("says Today / Yesterday with the time", () => {
    expect(formatItemDate(new Date(2026, 8, 24, 8, 40).toISOString(), now)).toBe("Today 08:40");
    expect(formatItemDate(new Date(2026, 8, 23, 18, 2).toISOString(), now)).toBe("Yesterday 18:02");
  });

  it("shows day and month, with the year only when it differs", () => {
    expect(formatItemDate(new Date(2026, 8, 22, 10).toISOString(), now)).toBe("22 Sep");
    expect(formatItemDate(new Date(2025, 0, 3, 10).toISOString(), now)).toBe("3 Jan 2025");
  });

  it("keeps unparseable dates as written", () => {
    expect(formatItemDate("yesterday", now)).toBe("yesterday");
    expect(formatItemDate("", now)).toBe("");
  });
});

describe("progressPercent", () => {
  it("clamps and handles unknown totals", () => {
    expect(progressPercent(30, 120)).toBe(25);
    expect(progressPercent(130, 120)).toBe(100);
    expect(progressPercent(10, 0)).toBe(0);
  });
});

describe("baseName / fileManagerName", () => {
  it("takes the last component for either separator", () => {
    expect(baseName("/Users/a/memo.m4a")).toBe("memo.m4a");
    expect(baseName("C:\\rec\\call.wav")).toBe("call.wav");
    expect(baseName("x.wav")).toBe("x.wav");
  });

  it("names the platform file manager", () => {
    expect(fileManagerName("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)")).toBe("Finder");
    expect(fileManagerName("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("Explorer");
    expect(fileManagerName("Mozilla/5.0 (X11; Linux x86_64)")).toBe("file manager");
  });
});
