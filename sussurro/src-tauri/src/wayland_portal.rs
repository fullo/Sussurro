//! Wayland text injection via the XDG RemoteDesktop portal (issue #40).
//!
//! KWin and Mutter deliberately do not expose the `virtual-keyboard` Wayland
//! protocol to external clients, so on stock KDE/GNOME Wayland the whole
//! wtype → enigo ladder silently no-ops. The RemoteDesktop portal is the
//! sanctioned path: no daemon, no uinput permissions, sandbox-friendly.
//!
//! The OS shows a consent dialog on first use; `PersistMode::ExplicitlyRevoked`
//! plus a persisted restore token keeps later runs silent. The token is
//! SINGLE-USE — every `Start` returns a fresh one that must replace the stored
//! value. (KDE currently forgets grants across reboots — kde#480235 — so KDE
//! users may re-approve after a reboot; within a session it holds.)
//!
//! One session is created lazily and reused for the whole app lifetime;
//! a session per paste would re-prompt every time.

use anyhow::{Context, Result};
use ashpd::desktop::remote_desktop::{
    CreateSessionOptions, DeviceType, KeyState, NotifyKeyboardKeysymOptions, RemoteDesktop,
    SelectDevicesOptions, StartOptions,
};
use ashpd::desktop::{PersistMode, Session};
use ashpd::enumflags2::BitFlags;
use std::path::PathBuf;
use std::sync::OnceLock;

/// X11 keysym for a character: printable Latin-1 maps to itself, everything
/// else to the Unicode keysym range (0x01000000 + codepoint). Control
/// characters we care about get their dedicated keysyms; the rest are
/// dropped (None) rather than typed as garbage.
pub fn keysym_for(c: char) -> Option<u32> {
    match c {
        '\n' => Some(0xff0d), // Return
        '\t' => Some(0xff09), // Tab
        c if ('\u{20}'..='\u{7e}').contains(&c) => Some(c as u32),
        c if ('\u{a0}'..='\u{ff}').contains(&c) => Some(c as u32),
        c if (c as u32) >= 0x100 => Some(0x0100_0000 + c as u32),
        _ => None,
    }
}

static TOKEN_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Where the single-use restore token is persisted. Call once at startup;
/// without it the consent dialog reappears on every app launch.
pub fn init(token_path: PathBuf) {
    let _ = TOKEN_PATH.set(token_path);
}

fn read_token() -> Option<String> {
    let path = TOKEN_PATH.get()?;
    std::fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn store_token(token: Option<&str>) {
    let Some(path) = TOKEN_PATH.get() else { return };
    match token {
        Some(t) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, t);
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The injection paths are called from plain (blocking) pipeline threads, so
/// the portal gets its own small runtime instead of borrowing Tauri's.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("portal runtime")
    })
}

struct Portal {
    proxy: RemoteDesktop,
    session: Session<RemoteDesktop>,
}

fn portal_slot() -> &'static tokio::sync::Mutex<Option<Portal>> {
    static PORTAL: OnceLock<tokio::sync::Mutex<Option<Portal>>> = OnceLock::new();
    PORTAL.get_or_init(|| tokio::sync::Mutex::new(None))
}

async fn connect() -> Result<Portal> {
    let proxy = RemoteDesktop::new()
        .await
        .context("RemoteDesktop portal unavailable (is xdg-desktop-portal running?)")?;
    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .context("portal CreateSession failed")?;
    let mut options = SelectDevicesOptions::default()
        .set_devices(BitFlags::from(DeviceType::Keyboard))
        .set_persist_mode(PersistMode::ExplicitlyRevoked);
    let token = read_token();
    if let Some(t) = token.as_deref() {
        options = options.set_restore_token(t);
    }
    proxy
        .select_devices(&session, options)
        .await
        .context("portal SelectDevices failed")?
        .response()
        .context("portal SelectDevices rejected")?;
    let devices = proxy
        .start(&session, None, StartOptions::default())
        .await
        .context("portal Start failed")?
        .response()
        .context("keyboard access not granted (portal consent dialog)")?;
    // Single-use token: whatever Start returned replaces the stored one.
    store_token(devices.restore_token());
    Ok(Portal { proxy, session })
}

async fn press(portal: &Portal, keysym: u32) -> Result<()> {
    portal
        .proxy
        .notify_keyboard_keysym(
            &portal.session,
            keysym as i32,
            KeyState::Pressed,
            NotifyKeyboardKeysymOptions::default(),
        )
        .await?;
    portal
        .proxy
        .notify_keyboard_keysym(
            &portal.session,
            keysym as i32,
            KeyState::Released,
            NotifyKeyboardKeysymOptions::default(),
        )
        .await?;
    Ok(())
}

async fn type_keysyms(portal: &Portal, keysyms: &[u32]) -> Result<()> {
    for &keysym in keysyms {
        press(portal, keysym).await?;
    }
    Ok(())
}

const CONTROL_L: u32 = 0xffe3;

async fn combo_keysym(portal: &Portal, letter_keysym: u32) -> Result<()> {
    portal
        .proxy
        .notify_keyboard_keysym(
            &portal.session,
            CONTROL_L as i32,
            KeyState::Pressed,
            NotifyKeyboardKeysymOptions::default(),
        )
        .await?;
    let result = press(portal, letter_keysym).await;
    // Always release the modifier, even if the letter failed.
    let released = portal
        .proxy
        .notify_keyboard_keysym(
            &portal.session,
            CONTROL_L as i32,
            KeyState::Released,
            NotifyKeyboardKeysymOptions::default(),
        )
        .await;
    result?;
    released.map_err(Into::into)
}

/// Type `text` into the focused app through the portal — direct typing, so it
/// works in terminals too and never touches the clipboard.
pub fn type_text(text: &str) -> Result<()> {
    let keysyms: Vec<u32> = text.chars().filter_map(keysym_for).collect();
    runtime().block_on(async {
        let mut slot = portal_slot().lock().await;
        if slot.is_none() {
            *slot = Some(connect().await?);
        }
        if type_keysyms(slot.as_ref().expect("session"), &keysyms).await.is_ok() {
            return Ok(());
        }
        // Session died mid-use (compositor restart, revoked grant):
        // reconnect once and retry.
        *slot = Some(connect().await?);
        type_keysyms(slot.as_ref().expect("session"), &keysyms).await
    })
}

/// Ctrl+<letter> (e.g. Ctrl+V / Ctrl+C) through the portal.
pub fn key_combo(letter: char) -> Result<()> {
    let letter_keysym = keysym_for(letter).context("unsupported combo letter")?;
    runtime().block_on(async {
        let mut slot = portal_slot().lock().await;
        if slot.is_none() {
            *slot = Some(connect().await?);
        }
        if combo_keysym(slot.as_ref().expect("session"), letter_keysym).await.is_ok() {
            return Ok(());
        }
        *slot = Some(connect().await?);
        combo_keysym(slot.as_ref().expect("session"), letter_keysym).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_maps_to_itself() {
        assert_eq!(keysym_for('a'), Some(0x61));
        assert_eq!(keysym_for(' '), Some(0x20));
        assert_eq!(keysym_for('~'), Some(0x7e));
    }

    #[test]
    fn newline_and_tab_use_dedicated_keysyms() {
        assert_eq!(keysym_for('\n'), Some(0xff0d));
        assert_eq!(keysym_for('\t'), Some(0xff09));
    }

    #[test]
    fn latin1_and_unicode_map_correctly() {
        assert_eq!(keysym_for('è'), Some(0xe8)); // Latin-1: itself
        assert_eq!(keysym_for('€'), Some(0x0100_0000 + 0x20ac));
        assert_eq!(keysym_for('日'), Some(0x0100_0000 + '日' as u32));
    }

    #[test]
    fn other_control_chars_are_dropped() {
        assert_eq!(keysym_for('\r'), None);
        assert_eq!(keysym_for('\u{7}'), None);
    }
}
