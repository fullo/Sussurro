import { useRef, useState } from "react";
import { comboFromEvent, hotkeyParts } from "../lib/hotkey";

/* ---------- Hotkey recorder widget ---------- */

export function HotkeyRecorder({
  value,
  onChange,
}: {
  value: string;
  onChange: (combo: string) => void;
}) {
  const [capturing, setCapturing] = useState(false);
  const btnRef = useRef<HTMLButtonElement>(null);

  const parts = hotkeyParts(value, navigator.platform.includes("Mac"));

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!capturing) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape") {
      setCapturing(false);
      return;
    }
    const combo = comboFromEvent(e);
    if (combo) {
      setCapturing(false);
      onChange(combo);
      btnRef.current?.blur();
    }
  };

  return (
    <button
      ref={btnRef}
      type="button"
      className={`hotkey-recorder${capturing ? " capturing" : ""}`}
      onClick={() => setCapturing(true)}
      onKeyDown={onKeyDown}
      onBlur={() => setCapturing(false)}
      aria-label={capturing ? "Press the new shortcut, Esc to cancel" : `Shortcut: ${value}. Click to change`}
    >
      {capturing ? (
        <span className="hotkey-hint">Press keys… <em>Esc to cancel</em></span>
      ) : (
        <span className="hotkey-keys">
          {parts.map((p, i) => (
            <kbd key={i}>{p}</kbd>
          ))}
        </span>
      )}
    </button>
  );
}
