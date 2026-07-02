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
/// Ctrl/Cmd+V → restore clipboard. Paste-injection works in far more apps than
/// per-character typing (and is what Handy ships); revisit for Wayland later.
pub fn inject_text(text: &str) -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("open clipboard")?;
    let saved = clipboard.get_text().ok();

    clipboard
        .set_text(text.to_string())
        .context("set clipboard")?;
    // Give the OS clipboard a beat to propagate before pasting.
    std::thread::sleep(Duration::from_millis(120));

    let paste_result = synth_paste();
    #[cfg(target_os = "linux")]
    let paste_result = paste_result.or_else(|_| wayland_type_fallback(text));
    paste_result?;

    // Let the target app read the clipboard before we restore it.
    std::thread::sleep(Duration::from_millis(200));
    if let Some(prev) = saved {
        let _ = clipboard.set_text(prev);
    }
    Ok(())
}

/// Synthesize Ctrl/Cmd+V in the focused app.
fn synth_paste() -> Result<()> {
    let mut enigo = Enigo::new(&EnigoSettings::default()).context("init enigo")?;
    let modifier = paste_modifier();
    enigo.key(modifier, Direction::Press).context("modifier down")?;
    enigo.key(Key::Unicode('v'), Direction::Click).context("press V")?;
    enigo.key(modifier, Direction::Release).context("modifier up")?;
    Ok(())
}

/// Wayland blocks cross-app synthetic input; `wtype` (virtual-keyboard
/// protocol) types the text directly when available.
#[cfg(target_os = "linux")]
fn wayland_type_fallback(text: &str) -> Result<()> {
    anyhow::ensure!(
        std::env::var("WAYLAND_DISPLAY").is_ok(),
        "not a Wayland session"
    );
    let status = std::process::Command::new("wtype")
        .arg(text)
        .status()
        .context("paste failed and wtype is not installed (needed on Wayland)")?;
    anyhow::ensure!(status.success(), "wtype exited with {status}");
    Ok(())
}

/// Copy the current selection via Ctrl/Cmd+C and return it (None when nothing
/// is selected). The selection stays active, so pasting right after replaces
/// it — exactly what command mode needs.
pub fn copy_selection() -> Result<Option<String>> {
    let mut clipboard = arboard::Clipboard::new().context("open clipboard")?;
    let before = clipboard.get_text().ok();
    let _ = clipboard.clear();

    let mut enigo = Enigo::new(&EnigoSettings::default()).context("init enigo")?;
    let modifier = paste_modifier();
    enigo.key(modifier, Direction::Press).context("modifier down")?;
    enigo.key(Key::Unicode('c'), Direction::Click).context("press C")?;
    enigo.key(modifier, Direction::Release).context("modifier up")?;
    std::thread::sleep(Duration::from_millis(250));

    let text = clipboard.get_text().ok().filter(|t| !t.trim().is_empty());
    if text.is_none() {
        // Nothing selected: put the user's clipboard back.
        if let Some(prev) = before {
            let _ = clipboard.set_text(prev);
        }
    }
    Ok(text)
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
