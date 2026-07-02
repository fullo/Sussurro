use anyhow::{anyhow, Result};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

/// Replace registered shortcuts: dictation (required) + command mode
/// (best-effort: a broken command combo must not break dictation).
pub fn apply(app: &AppHandle, dictation: &str, command: &str) -> Result<()> {
    let shortcut: Shortcut = dictation
        .parse()
        .map_err(|e| anyhow!("invalid hotkey '{dictation}': {e:?}"))?;
    app.global_shortcut().unregister_all()?;
    app.global_shortcut().register(shortcut)?;
    if !command.trim().is_empty() && command != dictation {
        if let Ok(cmd) = command.parse::<Shortcut>() {
            let _ = app.global_shortcut().register(cmd);
        }
    }
    Ok(())
}
