/* New → System audio + mic (#139): the pure logic behind the tab — which
   device to preselect, how devices are labelled, what is wrong with a
   choice, and the per-OS setup help. The backend checks the same rules
   (sources/system.rs `validate_devices`); these only keep the UI honest.
   Since #140 the first choice can be the OS's own capture of the computer's
   sound (`NATIVE`), with the device picker as the fallback. */

import type { InputDeviceInfo, NativeLoopback, SystemAudioDevices } from "./types";

/** The picker value of "This computer's sound (built-in)" (#140) — not a
 *  device name any OS uses. */
export const NATIVE = "::sussurro-native::";
export const NATIVE_LABEL = "This computer's sound (built-in)";

/** Shown in the tab whatever the choice: no echo cancellation (#140). */
export const ECHO_NOTE =
  "On speakers your microphone hears the others too, so their words can end up on your channel as well. Use headphones — Sussurro does not cancel echo.";

/** Whether the native choice is offered. */
export function nativeAvailable(list: SystemAudioDevices | null): boolean {
  return !!list?.native?.available;
}

/** The native choice's label in the picker. */
export function nativeOptionLabel(native: NativeLoopback): string {
  return native.detail ? `${NATIVE_LABEL} · ${native.detail}` : NATIVE_LABEL;
}

/** What to know about the native capture on this OS, once chosen. */
export function nativeNote(native: NativeLoopback): string {
  switch (native.backend) {
    case "coreaudio-tap":
      return "Records every app's sound except Sussurro's. The first time, macOS asks to allow Sussurro to record system audio — or turn it on in System Settings → Privacy & Security → Screen & System Audio Recording.";
    case "wasapi":
      return "Records what plays on the default output device (WASAPI loopback). If you switch the output mid-call, start a new recording.";
    case "pulse-monitor":
      return "Records the monitor of the default output (PulseAudio / PipeWire, through parec). If you switch the output mid-call, start a new recording.";
    default:
      return "Records what the computer plays.";
  }
}

/** Why the native choice is not offered, or null (offered, or not known). */
export function nativeFallbackNote(list: SystemAudioDevices | null): string | null {
  const n = list?.native;
  if (!n || n.available) return null;
  return `${NATIVE_LABEL} is not available here: ${n.reason ?? "unknown reason"}.`;
}

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

/** The system device to preselect: the remembered choice if it is still
 *  there (a connected device, or the native capture while available), else
 *  the native capture, else the first loopback-looking device that is not
 *  the mic, else nothing (the user must choose). */
export function initialSystemDevice(list: SystemAudioDevices, remembered: string | null, mic: string): string {
  const names = list.devices.map((d) => d.name);
  const theMic = micName(mic, list.default_input);
  const native = nativeAvailable(list);
  if (remembered === NATIVE && native) return NATIVE;
  if (remembered && names.includes(remembered) && remembered !== theMic) return remembered;
  if (native) return NATIVE;
  return list.devices.find((d) => d.loopback && d.name !== theMic)?.name ?? "";
}

/** Whether `choice` is still a valid pick in a fresh listing. */
export function choiceStillValid(list: SystemAudioDevices, choice: string): boolean {
  if (choice === NATIVE) return nativeAvailable(list);
  return !!choice && list.devices.some((d) => d.name === choice);
}

/** A device's label in the pickers. */
export function deviceLabel(d: InputDeviceInfo, defaultInput: string | null): string {
  const tags = [d.name === defaultInput ? "default input" : "", d.loopback ? "looks like a loopback device" : ""].filter(Boolean);
  return tags.length ? `${d.name} · ${tags.join(" · ")}` : d.name;
}

/** Why the chosen pair can't start, or null. `native`: the listing's
 *  native capture, for a `NATIVE` choice. */
export function devicesProblem(
  mic: string,
  system: string,
  defaultInput: string | null,
  native: NativeLoopback | null = null,
): string | null {
  if (system === NATIVE) {
    return native?.available ? null : `${NATIVE_LABEL} is not available here: ${native?.reason ?? "unknown reason"}.`;
  }
  if (!system.trim()) return "Choose the system audio device.";
  if (micName(mic, defaultInput) === system) return "The system audio device is the microphone — choose a different device for one of them.";
  return null;
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
