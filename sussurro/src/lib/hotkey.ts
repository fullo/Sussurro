/** The key fields of a keyboard event that a hotkey is built from. */
export interface KeyLike {
  code: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/** Map a KeyboardEvent to the tauri-plugin-global-shortcut string, or null if incomplete. */
export function comboFromEvent(e: KeyLike): string | null {
  const code = e.code;
  // Ignore presses of bare modifier keys — wait for the main key.
  if (/^(Control|Shift|Alt|Meta)(Left|Right)?$/.test(code)) return null;

  let key: string | null = null;
  if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
  else if (/^Digit[0-9]$/.test(code)) key = code.slice(5);
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) key = code;
  else if (code === "Space") key = "Space";
  else if (/^Arrow(Up|Down|Left|Right)$/.test(code)) key = code.slice(5);
  else if (
    ["Comma", "Period", "Slash", "Semicolon", "Quote", "Minus", "Equal",
     "Backquote", "BracketLeft", "BracketRight", "Backslash", "Home", "End",
     "PageUp", "PageDown", "Insert", "Enter", "Tab"].includes(code)
  ) key = code;
  if (!key) return null;

  const mods: string[] = [];
  if (e.ctrlKey || e.metaKey) mods.push("CommandOrControl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");

  // A bare letter/digit as a global hotkey would hijack normal typing.
  const isFKey = /^F\d+$/.test(key);
  if (mods.length === 0 && !isFKey) return null;

  return [...mods, key].join("+");
}

/** Display parts of a shortcut string: CommandOrControl → ⌘ on macOS, Ctrl elsewhere. */
export function hotkeyParts(value: string, isMac: boolean): string[] {
  return value.split("+").map((p) => (p === "CommandOrControl" ? (isMac ? "⌘" : "Ctrl") : p));
}
