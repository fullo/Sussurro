import { describe, expect, it } from "vitest";
import {
  deviceLabel,
  devicesProblem,
  initialSystemDevice,
  micName,
  osOf,
  SETUP_HELP,
  systemTabVisible,
} from "./systemAudio";
import type { SystemAudioDevices } from "./types";

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
