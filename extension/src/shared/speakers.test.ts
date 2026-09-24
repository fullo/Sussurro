import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { chipFor, lineSpeakerId, VOICE_COLORS, voiceColor, voiceNumber, YOU_COLOR } from "./speakers";

const DOC_RS = new URL("../../../sussurro/src-tauri/src/speakers/doc.rs", import.meta.url);

describe("speaker chips", () => {
  it("use the app's palette (speakers/doc.rs)", () => {
    const rs = readFileSync(DOC_RS, "utf8");
    const voices = /pub const VOICE_COLORS: \[&str; \d+\] = \[([^\]]+)\]/.exec(rs)?.[1];
    expect(voices, "VOICE_COLORS in doc.rs").toBeTruthy();
    expect([...voices!.matchAll(/"(#[0-9a-f]+)"/gi)].map((m) => m[1])).toEqual(VOICE_COLORS);
    expect(/pub const YOU_COLOR: &str = "([^"]+)"/.exec(rs)?.[1]).toBe(YOU_COLOR);
  });

  it("label and colour the app's speakers like the app", () => {
    expect(chipFor("you")).toEqual({ id: "you", label: "You", color: "#1a1a1a" });
    expect(chipFor("voice:1")).toEqual({ id: "voice:1", label: "Voice 1", color: "#0f766e" });
    expect(chipFor("voice:9").color).toBe(voiceColor(1));
    expect(chipFor("meet:Anna Rossi").label).toBe("Anna Rossi");
    expect(chipFor("meet:Anna").color).toBe(chipFor("meet:Anna").color);
    expect(VOICE_COLORS).toContain(chipFor("meet:Anna").color);
  });

  it("prefer what the app said, with a valid colour only", () => {
    expect(chipFor("voice:2", { label: "Bo" })).toEqual({ id: "voice:2", label: "Bo", color: "#7e22ce" });
    expect(chipFor("voice:2", { label: " ", color: "#123456" })).toEqual({ id: "voice:2", label: "Voice 2", color: "#123456" });
    expect(chipFor("voice:2", { label: "Bo", color: "red;background:url(x)" }).color).toBe("#7e22ce");
  });

  it("read voice numbers and fall back to You on the mic channel", () => {
    expect(voiceNumber("voice:12")).toBe(12);
    expect(voiceNumber("voice:0")).toBeNull();
    expect(voiceNumber("voice:new")).toBeNull();
    expect(lineSpeakerId({ channel: "mic" })).toBe("you");
    expect(lineSpeakerId({ channel: "remote" })).toBeUndefined();
    expect(lineSpeakerId({ channel: "mic", speaker_id: "voice:1" })).toBe("voice:1");
  });
});
