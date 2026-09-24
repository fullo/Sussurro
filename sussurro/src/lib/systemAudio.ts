/* New → System audio + mic (#139): the pure logic behind the tab — which
   device to preselect, how devices are labelled, what is wrong with a
   choice, and the per-OS setup help. The backend checks the same rules
   (sources/system.rs `validate_devices`); these only keep the UI honest. */

import type { InputDeviceInfo, SystemAudioDevices } from "./types";

export type Os = "mac" | "windows" | "linux";

/** The OS from a user agent string (the webview's). */
export function osOf(userAgent: string = typeof navigator !== "undefined" ? navigator.userAgent : ""): Os {
  if (/Mac/i.test(userAgent)) return "mac";
  if (/Win/i.test(userAgent)) return "windows";
  return "linux";
}

/** How to get the computer's output into an input device, per OS. */
export const SETUP_HELP: Record<Os, { devices: string; steps: string[] }> = {
  mac: {
    devices: "BlackHole (free) or Loopback",
    steps: [
      "Install BlackHole 2ch (brew install blackhole-2ch) or Rogue Amoeba's Loopback.",
      "In Audio MIDI Setup create a Multi-Output Device with your speakers or headphones and BlackHole, and make it the sound output — you keep hearing the call.",
      "Choose BlackHole 2ch below as the system audio device.",
    ],
  },
  windows: {
    devices: "VB-Cable or Voicemeeter",
    steps: [
      "Install VB-Cable (or Voicemeeter) from vb-audio.com.",
      "Send the meeting app's speaker (or the Windows output) to CABLE Input; to keep hearing it, tick Listen to this device on CABLE Output (Sound → Recording → Properties → Listen).",
      "Choose CABLE Output below as the system audio device. Some sound cards also offer Stereo Mix.",
    ],
  },
  linux: {
    devices: "a PulseAudio / PipeWire monitor source",
    steps: [
      "Every output has a monitor source that carries what it plays.",
      "Expose it to Sussurro as an ALSA device (a pcm of type pulse on the .monitor source, with a hint so it is listed — see docs/compile/linux.md), or pick the pulse / pipewire device and route that recording stream to Monitor of … in pavucontrol.",
      "Choose it below as the system audio device.",
    ],
  },
};

/** The name the microphone choice records: the chosen device, or the
 *  default input for "" (the dictation's device when that is empty too). */
export function micName(mic: string, defaultInput: string | null): string {
  return mic.trim() || defaultInput || "";
}

/** The system device to preselect: the remembered one if it is still
 *  connected, else the first loopback-looking device that is not the mic,
 *  else nothing (the user must choose). */
export function initialSystemDevice(list: SystemAudioDevices, remembered: string | null, mic: string): string {
  const names = list.devices.map((d) => d.name);
  const theMic = micName(mic, list.default_input);
  if (remembered && names.includes(remembered) && remembered !== theMic) return remembered;
  return list.devices.find((d) => d.loopback && d.name !== theMic)?.name ?? "";
}

/** A device's label in the pickers. */
export function deviceLabel(d: InputDeviceInfo, defaultInput: string | null): string {
  const tags = [d.name === defaultInput ? "default input" : "", d.loopback ? "looks like a loopback device" : ""].filter(Boolean);
  return tags.length ? `${d.name} · ${tags.join(" · ")}` : d.name;
}

/** Why the chosen pair can't start, or null. */
export function devicesProblem(mic: string, system: string, defaultInput: string | null): string | null {
  if (!system.trim()) return "Choose the system audio device.";
  if (micName(mic, defaultInput) === system) return "The system audio device is the microphone — choose a different device for one of them.";
  return null;
}

/** Whether the tab is offered: behind the 0.9 meetings preview (it records
 *  other people, E12), and always while its session runs so it can be
 *  stopped even if the preview was switched off meanwhile. */
export function systemTabVisible(meetingsEnabled: boolean, running: boolean): boolean {
  return meetingsEnabled || running;
}

const KEY = "systemAudioDevice";

/** The last system device used (a per-window convenience). */
export function loadSystemDevice(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

export function saveSystemDevice(name: string): void {
  try {
    localStorage.setItem(KEY, name);
  } catch {
    /* private mode: not remembered */
  }
}
