import { describe, expect, it } from "vitest";
import { comboFromEvent, hotkeyParts, type KeyLike } from "./hotkey";
import { isLocalEndpoint, parseEndpoint } from "./endpoint";

const key = (code: string, mods: Partial<KeyLike> = {}): KeyLike => ({
  code,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...mods,
});

describe("comboFromEvent", () => {
  it("builds plugin shortcut strings", () => {
    expect(comboFromEvent(key("Space", { metaKey: true, shiftKey: true }))).toBe(
      "CommandOrControl+Shift+Space",
    );
    expect(comboFromEvent(key("KeyD", { ctrlKey: true, altKey: true }))).toBe(
      "CommandOrControl+Alt+D",
    );
    expect(comboFromEvent(key("F9"))).toBe("F9"); // function keys may stand alone
  });

  it("waits for a main key and refuses bare letters", () => {
    expect(comboFromEvent(key("ShiftLeft", { shiftKey: true }))).toBeNull();
    expect(comboFromEvent(key("KeyA"))).toBeNull();
    expect(comboFromEvent(key("Digit1"))).toBeNull();
    expect(comboFromEvent(key("IntlBackslash", { ctrlKey: true }))).toBeNull();
  });

  it("shows ⌘ on macOS and Ctrl elsewhere", () => {
    expect(hotkeyParts("CommandOrControl+Shift+Space", true)).toEqual(["⌘", "Shift", "Space"]);
    expect(hotkeyParts("CommandOrControl+Shift+Space", false)).toEqual(["Ctrl", "Shift", "Space"]);
  });
});

describe("cleanup endpoint (mirrors settings.rs)", () => {
  it("recognizes local hosts", () => {
    for (const url of ["http://localhost:11434", "localhost:11434", "http://[::1]:1", "http://my-mac.local"]) {
      expect(isLocalEndpoint(url)).toBe(true);
    }
  });

  it("flags remote and malformed endpoints", () => {
    for (const url of ["https://api.openai.com/v1", "http://192.168.1.50:8080", "", "http://"]) {
      expect(isLocalEndpoint(url)).toBe(false);
    }
    expect(parseEndpoint("https://user@Example.com:8443/v1")).toEqual({ host: "example.com", secure: true });
  });
});
