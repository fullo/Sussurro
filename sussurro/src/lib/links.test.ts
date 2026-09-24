import { describe, expect, it } from "vitest";
import {
  canTranscribeLink,
  describeDownload,
  downloadPercent,
  formatBytes,
  kindLabel,
  linkPhase,
  linkProblem,
  looksLikeLink,
} from "./links";
import type { EngineDownload, LinkInfo, YtDlpStatus } from "./types";

const dl = (downloaded_bytes: number, total_bytes: number | null, via: EngineDownload["via"] = "direct"): EngineDownload => ({
  session_id: 1,
  via,
  downloaded_bytes,
  total_bytes,
  title: null,
});

const info = (over: Partial<LinkInfo> = {}): LinkInfo => ({ kind: "direct", error: null, local: false, label: "example.com/a.mp3", ...over });
const yt = (found: boolean): YtDlpStatus => ({ found, path: found ? "/opt/homebrew/bin/yt-dlp" : null, version: null, install_help: "brew install yt-dlp" });

describe("link helpers (#123)", () => {
  it("asks the backend only about link-looking input", () => {
    expect(looksLikeLink("https://youtu.be/x")).toBe(true);
    expect(looksLikeLink("  ftp://x")).toBe(true); // refused by the backend, with a reason
    expect(looksLikeLink("youtube")).toBe(false);
    expect(looksLikeLink("")).toBe(false);
  });

  it("labels the detected kind", () => {
    expect(kindLabel("direct")).toBe("Direct media");
    expect(kindLabel("platform")).toBe("Video platform (yt-dlp)");
  });

  it("formats sizes and download progress", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(3.5 * 1024 * 1024)).toBe("3.5 MB");
    expect(formatBytes(NaN)).toBe("");
    expect(downloadPercent(dl(50, 200))).toBe(25);
    expect(downloadPercent(dl(50, null))).toBeNull();
    expect(downloadPercent(dl(500, 200))).toBe(100);
    expect(downloadPercent(null)).toBeNull();
    expect(describeDownload(dl(1024 * 1024, 4 * 1024 * 1024))).toBe("1.0 MB of 4.0 MB · 25%");
    expect(describeDownload(dl(2048, null))).toBe("2.0 KB");
    expect(describeDownload(null)).toBe("Connecting…");
    expect(describeDownload(dl(0, null, "yt-dlp"))).toMatch(/yt-dlp/);
  });

  it("shows the download phase until the engine opens the item", () => {
    expect(linkPhase({ itemId: null, progress: null })).toBe("download");
    expect(linkPhase({ itemId: "2026/09/x", progress: null })).toBe("transcribe");
  });

  it("enables Transcribe only for a usable link", () => {
    expect(canTranscribeLink(info(), null, false)).toBe(true);
    expect(canTranscribeLink(null, null, false)).toBe(false);
    expect(canTranscribeLink(info({ kind: null, error: "only http and https links are supported" }), null, false)).toBe(false);
    // A platform needs yt-dlp; while the check runs, the backend decides.
    expect(canTranscribeLink(info({ kind: "platform" }), yt(false), false)).toBe(false);
    expect(canTranscribeLink(info({ kind: "platform" }), yt(true), false)).toBe(true);
    expect(canTranscribeLink(info({ kind: "platform" }), null, false)).toBe(true);
    // A local address needs the explicit opt-in.
    expect(canTranscribeLink(info({ local: true }), null, false)).toBe(false);
    expect(canTranscribeLink(info({ local: true }), null, true)).toBe(true);
    expect(linkProblem(info({ local: true }), null, false)).toMatch(/Allow local network addresses/);
    expect(linkProblem(info({ kind: "platform" }), yt(false), false)).toMatch(/yt-dlp/);
    expect(linkProblem(info(), yt(false), false)).toBeNull();
  });
});
