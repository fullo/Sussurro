use anyhow::{anyhow, Result};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

/// Replace the registered global shortcut with the dictation hotkey.
pub fn apply(app: &AppHandle, dictation: &str) -> Result<()> {
    let shortcut: Shortcut = dictation
        .parse()
        .map_err(|e| anyhow!("invalid hotkey '{dictation}': {e:?}"))?;
    app.global_shortcut().unregister_all()?;
    app.global_shortcut().register(shortcut)?;
    Ok(())
}
