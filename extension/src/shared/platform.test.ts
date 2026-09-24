import { describe, expect, it } from "vitest";
import { detectPlatform } from "./platform";

describe("detectPlatform", () => {
  it("recognises the supported meeting pages", () => {
    expect(detectPlatform("https://meet.google.com/abc-defg-hij")).toBe("meet");
    expect(detectPlatform("https://teams.microsoft.com/v2/")).toBe("teams");
    expect(detectPlatform("https://teams.live.com/meet/123")).toBe("teams");
    expect(detectPlatform("https://app.zoom.us/wc/123456/join")).toBe("zoom");
    expect(detectPlatform("https://us05web.zoom.us/wc/123/start")).toBe("zoom");
    expect(detectPlatform("https://zoom.us/wc/join/123")).toBe("zoom");
  });

  it("ignores Zoom pages outside the web client", () => {
    expect(detectPlatform("https://zoom.us/pricing")).toBeNull();
    expect(detectPlatform("https://app.zoom.us/wcx/1")).toBeNull();
  });

  it("rejects look-alike hosts, other schemes and garbage", () => {
    expect(detectPlatform("https://meet.google.com.evil.example/abc")).toBeNull();
    expect(detectPlatform("https://evilzoom.us/wc/1")).toBeNull();
    expect(detectPlatform("http://meet.google.com/abc")).toBeNull();
    expect(detectPlatform("https://example.com/")).toBeNull();
    expect(detectPlatform("not a url")).toBeNull();
  });
});
