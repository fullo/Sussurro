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

    let mut enigo = Enigo::new(&EnigoSettings::default()).context("init enigo")?;
    let modifier = paste_modifier();
    enigo.key(modifier, Direction::Press).context("modifier down")?;
    enigo.key(Key::Unicode('v'), Direction::Click).context("press V")?;
    enigo.key(modifier, Direction::Release).context("modifier up")?;

    // Let the target app read the clipboard before we restore it.
    std::thread::sleep(Duration::from_millis(200));
    if let Some(prev) = saved {
        let _ = clipboard.set_text(prev);
    }
    Ok(())
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
