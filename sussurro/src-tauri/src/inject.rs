use anyhow::{Context, Result};
use enigo::{Direction, Enigo, Key, Keyboard, Settings as EnigoSettings};
use std::time::Duration;

pub fn paste_modifier() -> Key {
    if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    }
}

/// Paste `text` into the focused app: save clipboard → set text → synthesize
/// Ctrl/Cmd+V → restore clipboard. Paste-injection works in far more apps
/// than per-character typing. On Wayland the RemoteDesktop portal types the
/// text directly first (the only zero-setup path on KDE/GNOME — issue #40);
/// the native tool ladder and the clipboard flow remain as fallbacks.
pub fn inject_text(text: &str) -> Result<()> {
    #[cfg(all(target_os = "linux", feature = "wayland-portal"))]
    if wayland::is_wayland() && crate::wayland_portal::type_text(text).is_ok() {
        return Ok(()); // typed directly — clipboard untouched, terminals included
    }

    let saved = read_clipboard();

    let written = write_clipboard(text);
    // No working clipboard (e.g. bare Wayland without wl-clipboard):
    // last resort is typing the text directly.
    #[cfg(target_os = "linux")]
    if written.is_err() && wayland::is_wayland() {
        return wayland::type_text(text);
    }
    written?;
    // Give the OS clipboard a beat to propagate before pasting.
    std::thread::sleep(Duration::from_millis(120));

    synth_combo('v')?;

    // Let the target app read the clipboard before we restore it.
    std::thread::sleep(Duration::from_millis(200));
    if let Some(prev) = saved {
        let _ = write_clipboard(&prev);
    }
    Ok(())
}

/// Copy the current selection via Ctrl/Cmd+C and return it (None when nothing
/// is selected). The selection stays active, so pasting right after replaces
/// it — exactly what command mode needs.
pub fn copy_selection() -> Result<Option<String>> {
    let before = read_clipboard();
    clear_clipboard();

    synth_combo('c')?;
    std::thread::sleep(Duration::from_millis(250));

    let text = read_clipboard().filter(|t| !t.trim().is_empty());
    if text.is_none() {
        // Nothing selected: put the user's clipboard back.
        if let Some(prev) = before {
            let _ = write_clipboard(&prev);
        }
    }
    Ok(text)
}

/* ---------- clipboard (arboard, with wl-clipboard fallback on Wayland) ---------- */

fn read_clipboard() -> Option<String> {
    if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
        return Some(text);
    }
    #[cfg(target_os = "linux")]
    if wayland::is_wayland() {
        return wayland::wl_paste().ok().flatten();
    }
    None
}

fn write_clipboard(text: &str) -> Result<()> {
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
        Ok(()) => Ok(()),
        Err(e) => {
            #[cfg(target_os = "linux")]
            if wayland::is_wayland() {
                return wayland::wl_copy(text);
            }
            Err(anyhow::anyhow!("set clipboard: {e}"))
        }
    }
}

fn clear_clipboard() {
    let _ = arboard::Clipboard::new().and_then(|mut c| c.clear());
    #[cfg(target_os = "linux")]
    if wayland::is_wayland() {
        wayland::wl_clear();
    }
}

/* ---------- key synthesis ---------- */

/// Ctrl/Cmd+<letter> in the focused app. On Wayland the ladder is:
/// RemoteDesktop portal → native tools (wtype → ydotool) → enigo. The portal
/// is the only zero-setup path on KDE/GNOME (issue #40); enigo's Wayland
/// support can silently no-op but still helps XWayland apps.
fn synth_combo(letter: char) -> Result<()> {
    #[cfg(target_os = "linux")]
    if wayland::is_wayland() {
        #[cfg(feature = "wayland-portal")]
        if crate::wayland_portal::key_combo(letter).is_ok() {
            return Ok(());
        }
        return match wayland::synth_combo_native(letter) {
            Ok(()) => Ok(()),
            Err(native_err) => enigo_combo(letter).map_err(|_| native_err),
        };
    }
    enigo_combo(letter)
}

/// macOS: enigo's Unicode→keycode lookup calls Text Input Source (TSM) APIs
/// that assert they run on the main thread; off-thread they abort the process
/// with SIGTRAP (#48). Injection runs on a background pipeline thread, so hop
/// the synthesis onto the main thread and block until it reports back — the
/// clipboard save/paste/restore sequence must stay ordered.
#[cfg(target_os = "macos")]
fn enigo_combo(letter: char) -> Result<()> {
    let Some(app) = crate::app_handle() else {
        return enigo_combo_inner(letter); // no app (unit tests) — best effort
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(enigo_combo_inner(letter));
    })
    .context("dispatch key synthesis to the main thread")?;
    rx.recv()
        .context("main-thread key synthesis did not report back")?
}

#[cfg(not(target_os = "macos"))]
fn enigo_combo(letter: char) -> Result<()> {
    enigo_combo_inner(letter)
}

fn enigo_combo_inner(letter: char) -> Result<()> {
    let mut enigo = Enigo::new(&EnigoSettings::default()).context("init enigo")?;
    let modifier = paste_modifier();
    enigo.key(modifier, Direction::Press).context("modifier down")?;
    enigo
        .key(Key::Unicode(letter), Direction::Click)
        .context("press key")?;
    enigo.key(modifier, Direction::Release).context("modifier up")?;
    Ok(())
}

/* ---------- native Wayland tools ---------- */

#[cfg(target_os = "linux")]
mod wayland {
    use anyhow::{anyhow, Context, Result};
    use std::io::Write;
    use std::process::{Command, Stdio};

    pub fn is_wayland() -> bool {
        std::env::var("WAYLAND_DISPLAY").is_ok()
            || std::env::var("XDG_SESSION_TYPE")
                .map(|v| v == "wayland")
                .unwrap_or(false)
    }

    fn available(cmd: &str) -> bool {
        Command::new("which")
            .arg(cmd)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Injection tools in preference order. wtype speaks the
    /// virtual-keyboard protocol (wlroots compositors, KDE); ydotool works
    /// everywhere — GNOME included — via uinput, but needs the ydotoold
    /// daemon running.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Tool {
        Wtype,
        Ydotool,
    }

    /// Pure: which tools to try given what's installed.
    pub fn plan(wtype: bool, ydotool: bool) -> Vec<Tool> {
        let mut tools = Vec::new();
        if wtype {
            tools.push(Tool::Wtype);
        }
        if ydotool {
            tools.push(Tool::Ydotool);
        }
        tools
    }

    /// Pure: the command line for Ctrl+<letter> per tool.
    /// ydotool uses Linux input-event codes: LEFTCTRL=29, C=46, V=47.
    pub fn combo_args(tool: Tool, letter: char) -> Option<(&'static str, Vec<String>)> {
        match tool {
            Tool::Wtype => Some((
                "wtype",
                vec![
                    "-M".into(),
                    "ctrl".into(),
                    "-P".into(),
                    letter.to_string(),
                    "-p".into(),
                    letter.to_string(),
                    "-m".into(),
                    "ctrl".into(),
                ],
            )),
            Tool::Ydotool => {
                let code = match letter {
                    'c' => 46,
                    'v' => 47,
                    _ => return None,
                };
                Some((
                    "ydotool",
                    vec![
                        "key".into(),
                        "29:1".into(),
                        format!("{code}:1"),
                        format!("{code}:0"),
                        "29:0".into(),
                    ],
                ))
            }
        }
    }

    pub fn synth_combo_native(letter: char) -> Result<()> {
        let tools = plan(available("wtype"), available("ydotool"));
        if tools.is_empty() {
            anyhow::bail!(
                "no Wayland injection tool found — install wtype (wlroots/KDE) \
                 or ydotool + ydotoold (any compositor, GNOME included)"
            );
        }
        let mut last_err = anyhow!("no tool attempted");
        for tool in tools {
            let Some((program, args)) = combo_args(tool, letter) else {
                continue;
            };
            match Command::new(program).args(&args).status() {
                Ok(status) if status.success() => return Ok(()),
                Ok(status) => last_err = anyhow!("{program} exited with {status}"),
                Err(e) => last_err = anyhow!("{program}: {e}"),
            }
        }
        Err(last_err)
    }

    /// Type the text directly (no clipboard). Slower, but works when no
    /// clipboard tool is available.
    pub fn type_text(text: &str) -> Result<()> {
        let status = Command::new("wtype")
            .arg("--")
            .arg(text)
            .status()
            .context("typing fallback failed: wtype not installed")?;
        anyhow::ensure!(status.success(), "wtype exited with {status}");
        Ok(())
    }

    pub fn wl_copy(text: &str) -> Result<()> {
        let mut child = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .spawn()
            .context("wl-copy not found — install wl-clipboard")?;
        child
            .stdin
            .as_mut()
            .context("wl-copy stdin")?
            .write_all(text.as_bytes())?;
        let status = child.wait()?;
        anyhow::ensure!(status.success(), "wl-copy exited with {status}");
        Ok(())
    }

    pub fn wl_paste() -> Result<Option<String>> {
        let out = Command::new("wl-paste")
            .arg("--no-newline")
            .output()
            .context("wl-paste not found — install wl-clipboard")?;
        if !out.status.success() {
            return Ok(None); // empty clipboard exits non-zero
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        Ok(if text.is_empty() { None } else { Some(text) })
    }

    pub fn wl_clear() {
        let _ = Command::new("wl-copy")
            .arg("--clear")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn plan_prefers_wtype_then_ydotool() {
            assert_eq!(plan(true, true), vec![Tool::Wtype, Tool::Ydotool]);
            assert_eq!(plan(true, false), vec![Tool::Wtype]);
            assert_eq!(plan(false, true), vec![Tool::Ydotool]);
            assert!(plan(false, false).is_empty());
        }

        #[test]
        fn wtype_combo_presses_and_releases_in_order() {
            let (program, args) = combo_args(Tool::Wtype, 'v').unwrap();
            assert_eq!(program, "wtype");
            assert_eq!(args, vec!["-M", "ctrl", "-P", "v", "-p", "v", "-m", "ctrl"]);
        }

        #[test]
        fn ydotool_combo_uses_input_event_codes() {
            let (program, args) = combo_args(Tool::Ydotool, 'c').unwrap();
            assert_eq!(program, "ydotool");
            assert_eq!(args, vec!["key", "29:1", "46:1", "46:0", "29:0"]);
            let (_, args) = combo_args(Tool::Ydotool, 'v').unwrap();
            assert_eq!(args, vec!["key", "29:1", "47:1", "47:0", "29:0"]);
            assert!(combo_args(Tool::Ydotool, 'x').is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_shortcut_is_ctrl_v_except_macos() {
        if cfg!(target_os = "macos") {
            assert!(matches!(paste_modifier(), enigo::Key::Meta));
        } else {
            assert!(matches!(paste_modifier(), enigo::Key::Control));
        }
    }

    /// Injects into whatever has focus — run manually with a text editor focused:
    /// cargo test inject_hello -- --ignored
    /// You have 3 seconds after starting the test to click into an editor.
    #[test]
    #[ignore]
    fn inject_hello() {
        std::thread::sleep(std::time::Duration::from_secs(3));
        inject_text("Hello from Sussurro! ").unwrap();
    }
}
