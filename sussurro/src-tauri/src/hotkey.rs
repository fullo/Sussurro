use anyhow::{anyhow, Result};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

/// Replace whatever shortcut is registered with `hotkey`.
pub fn apply(app: &AppHandle, hotkey: &str) -> Result<()> {
    let shortcut: Shortcut = hotkey
        .parse()
        .map_err(|e| anyhow!("invalid hotkey '{hotkey}': {e:?}"))?;
    app.global_shortcut().unregister_all()?;
    app.global_shortcut().register(shortcut)?;
    Ok(())
}
