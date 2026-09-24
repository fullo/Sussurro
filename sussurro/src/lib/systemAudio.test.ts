import { describe, expect, it } from "vitest";
import {
  choiceStillValid,
  deviceLabel,
  devicesProblem,
  ECHO_NOTE,
  initialSystemDevice,
  micName,
  NATIVE,
  nativeAvailable,
  nativeFallbackNote,
  nativeNote,
  nativeOptionLabel,
  osOf,
  SETUP_HELP,
  systemTabVisible,
} from "./systemAudio";
import type { NativeLoopback, SystemAudioDevices } from "./types";

const list: SystemAudioDevices = {
  default_input: "MacBook Pro Microphone",
  devices: [
    { name: "BlackHole 2ch", loopback: true },
    { name: "Loopback Audio", loopback: true },
    { name: "MacBook Pro Microphone", loopback: false },
    { name: "USB Audio Device", loopback: false },
  ],
};

describe("system audio tab (#139)", () => {
  it("is gated by the meetings preview, but never hides a running session", () => {
    expect(systemTabVisible(false, false)).toBe(false);
    expect(systemTabVisible(true, false)).toBe(true);
    expect(systemTabVisible(false, true)).toBe(true);
  });

  it("preselects the remembered device, else the first loopback device that is not the mic", () => {
    expect(initialSystemDevice(list, "Loopback Audio", "")).toBe("Loopback Audio");
    expect(initialSystemDevice(list, "Gone Device", "")).toBe("BlackHole 2ch");
    expect(initialSystemDevice(list, null, "BlackHole 2ch")).toBe("Loopback Audio");
    // The remembered device is now the mic: not offered twice.
    expect(initialSystemDevice(list, "BlackHole 2ch", "BlackHole 2ch")).toBe("Loopback Audio");
    expect(initialSystemDevice({ default_input: null, devices: [{ name: "USB", loopback: false }] }, null, "")).toBe("");
  });

  it("labels the default input and loopback-looking devices", () => {
    expect(deviceLabel(list.devices[0], list.default_input)).toBe("BlackHole 2ch · looks like a loopback device");
    expect(deviceLabel(list.devices[2], list.default_input)).toBe("MacBook Pro Microphone · default input");
    expect(deviceLabel(list.devices[3], list.default_input)).toBe("USB Audio Device");
  });

  it("refuses a missing system device and the same device twice", () => {
    expect(micName("", "Built-in")).toBe("Built-in");
    expect(micName(" USB ", "Built-in")).toBe("USB");
    expect(devicesProblem("", "", "Built-in")).toMatch(/Choose/);
    expect(devicesProblem("", "BlackHole 2ch", "Built-in")).toBeNull();
    expect(devicesProblem("BlackHole 2ch", "BlackHole 2ch", "Built-in")).toMatch(/is the microphone/);
    expect(devicesProblem("", "Built-in", "Built-in")).toMatch(/is the microphone/);
  });

  it("tells the OS from the user agent and has setup help for each", () => {
    expect(osOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0)")).toBe("mac");
    expect(osOf("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("windows");
    expect(osOf("Mozilla/5.0 (X11; Linux x86_64)")).toBe("linux");
    for (const os of ["mac", "windows", "linux"] as const) {
      expect(SETUP_HELP[os].steps.length).toBeGreaterThan(1);
    }
  });
});

const tap: NativeLoopback = {
  available: true,
  backend: "coreaudio-tap",
  detail: "MacBook Pro Speakers",
  reason: null,
  needs_permission: true,
};
const tooOld: NativeLoopback = {
  available: false,
  backend: "coreaudio-tap",
  detail: null,
  reason: "recording the computer's sound directly needs macOS 14.2 or later (this Mac runs 13.6) — use a loopback device such as BlackHole",
  needs_permission: false,
};

describe("this computer's sound, built-in (#140)", () => {
  const withNative = { ...list, native: tap };
  const withoutNative = { ...list, native: tooOld };

  it("is preselected when available, unless another device was chosen before", () => {
    expect(initialSystemDevice(withNative, null, "")).toBe(NATIVE);
    expect(initialSystemDevice(withNative, NATIVE, "")).toBe(NATIVE);
    expect(initialSystemDevice(withNative, "Loopback Audio", "")).toBe("Loopback Audio");
    expect(initialSystemDevice(withNative, "Gone Device", "")).toBe(NATIVE);
  });

  it("falls back to the device picker when unavailable", () => {
    expect(nativeAvailable(withoutNative)).toBe(false);
    expect(nativeAvailable(list)).toBe(false);
    expect(initialSystemDevice(withoutNative, NATIVE, "")).toBe("BlackHole 2ch");
    expect(choiceStillValid(withoutNative, NATIVE)).toBe(false);
    expect(choiceStillValid(withNative, NATIVE)).toBe(true);
    expect(choiceStillValid(withNative, "BlackHole 2ch")).toBe(true);
    expect(choiceStillValid(withNative, "Gone Device")).toBe(false);
    expect(choiceStillValid(withNative, "")).toBe(false);
  });

  it("says why it is not offered", () => {
    expect(nativeFallbackNote(withoutNative)).toMatch(/not available here: .*14\.2/);
    expect(nativeFallbackNote(withNative)).toBeNull();
    expect(nativeFallbackNote(list)).toBeNull();
    expect(nativeFallbackNote(null)).toBeNull();
    expect(devicesProblem("", NATIVE, "Built-in", tap)).toBeNull();
    expect(devicesProblem("", NATIVE, "Built-in", tooOld)).toMatch(/14\.2/);
    expect(devicesProblem("", NATIVE, "Built-in", null)).toMatch(/not available/);
  });

  it("names what it records and what to know per OS", () => {
    expect(nativeOptionLabel(tap)).toBe("This computer's sound (built-in) · MacBook Pro Speakers");
    expect(nativeOptionLabel({ ...tap, detail: null })).toBe("This computer's sound (built-in)");
    expect(nativeNote(tap)).toMatch(/System Audio Recording/);
    expect(nativeNote({ ...tap, backend: "wasapi" })).toMatch(/default output/);
    expect(nativeNote({ ...tap, backend: "pulse-monitor" })).toMatch(/monitor/);
  });

  it("suggests headphones instead of echo cancellation", () => {
    expect(ECHO_NOTE).toMatch(/headphones/);
    expect(ECHO_NOTE).toMatch(/does not cancel echo/);
  });
});
