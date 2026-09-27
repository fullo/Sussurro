use anyhow::{anyhow, Result};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

fn parse(dictation: &str) -> Result<Shortcut> {
    dictation
        .parse()
        .map_err(|e| anyhow!("invalid hotkey '{dictation}': {e:?}"))
}

/// Replace the registered global shortcut with the dictation hotkey.
pub fn apply(app: &AppHandle, dictation: &str) -> Result<()> {
    let shortcut = parse(dictation)?;
    app.global_shortcut().unregister_all()?;
    app.global_shortcut().register(shortcut)?;
    Ok(())
}

/// A settings save: re-register only when the hotkey changed. An unchanged
/// hotkey never fails the save — when another app holds it (a second
/// Sussurro, another tool) every save used to be rejected, silently dropping
/// unrelated changes such as the archive folder; it is only retried here in
/// case it has been freed since. A new hotkey that can't be registered is an
/// error, and the previous one is put back so dictation keeps working.
pub fn change(app: &AppHandle, previous: &str, next: &str) -> Result<()> {
    let shortcut = parse(next)?;
    if previous == next {
        if !app.global_shortcut().is_registered(shortcut) {
            let _ = apply(app, next);
        }
        return Ok(());
    }
    if let Err(e) = apply(app, next) {
        let _ = apply(app, previous);
        return Err(e.context(format!("hotkey '{next}' is not available")));
    }
    Ok(())
}
