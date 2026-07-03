//! Cross-platform OS permission checks surfaced in the setup banner.
//!
//! - macOS: Microphone (TCC) and Accessibility (TCC, needed for paste
//!   injection) are real runtime queries.
//! - Windows: Microphone reflects the "let desktop apps access your
//!   microphone" privacy toggle; injection needs no permission.
//! - Linux: neither is gated per-app (ALSA/PulseAudio are open), so both
//!   report NotApplicable.

use serde::Serialize;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PermState {
    Granted,
    Denied,
    /// Not yet decided, or can't be determined without prompting.
    Unknown,
    /// This OS doesn't gate the capability behind a permission.
    NotApplicable,
}

#[derive(Serialize, Clone, Copy)]
pub struct Permissions {
    pub microphone: PermState,
    pub accessibility: PermState,
}

pub fn check() -> Permissions {
    #[cfg(target_os = "macos")]
    {
        Permissions {
            microphone: macos::microphone(),
            accessibility: macos::accessibility(),
        }
    }
    #[cfg(target_os = "windows")]
    {
        Permissions {
            microphone: windows::microphone(),
            accessibility: PermState::NotApplicable,
        }
    }
    #[cfg(target_os = "linux")]
    {
        Permissions {
            microphone: PermState::NotApplicable,
            accessibility: PermState::NotApplicable,
        }
    }
}

/// Open the OS privacy pane for a permission (`"microphone"` / `"accessibility"`).
pub fn open_settings(target: &str) -> std::io::Result<()> {
    let url = settings_url(target);
    if url.is_empty() {
        return Ok(());
    }
    open_url(url)
}

#[cfg(target_os = "macos")]
fn settings_url(target: &str) -> &'static str {
    match target {
        "microphone" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        "accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        _ => "",
    }
}

#[cfg(target_os = "windows")]
fn settings_url(target: &str) -> &'static str {
    match target {
        "microphone" => "ms-settings:privacy-microphone",
        _ => "",
    }
}

#[cfg(target_os = "linux")]
fn settings_url(_target: &str) -> &'static str {
    ""
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.spawn()?;
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use super::PermState;
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use objc2_foundation::NSString;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    pub fn accessibility() -> PermState {
        if unsafe { AXIsProcessTrusted() } {
            PermState::Granted
        } else {
            PermState::Denied
        }
    }

    pub fn microphone() -> PermState {
        #[link(name = "AVFoundation", kind = "framework")]
        extern "C" {
            static AVMediaTypeAudio: &'static NSString;
        }
        let Some(cls) = AnyClass::get(c"AVCaptureDevice") else {
            return PermState::Unknown;
        };
        let media: &NSString = unsafe { AVMediaTypeAudio };
        // +[AVCaptureDevice authorizationStatusForMediaType:] — reads status,
        // never prompts. 0=notDetermined 1=restricted 2=denied 3=authorized.
        let status: isize = unsafe { msg_send![cls, authorizationStatusForMediaType: media] };
        match status {
            3 => PermState::Granted,
            1 | 2 => PermState::Denied,
            _ => PermState::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercises the real OS queries (AXIsProcessTrusted / AVFoundation on
    // macOS, reg on Windows) to catch FFI/ABI breakage — must not panic.
    #[test]
    fn check_returns_without_panicking() {
        let p = check();
        let _ = (p.microphone, p.accessibility);
    }

    #[test]
    fn unknown_targets_open_nothing() {
        assert!(open_settings("nonsense").is_ok());
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::PermState;
    use std::process::Command;

    /// The per-user "let desktop apps access your microphone" consent lives in
    /// the registry; there is no prompt for classic desktop apps.
    pub fn microphone() -> PermState {
        let out = Command::new("reg")
            .args([
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone",
                "/v",
                "Value",
            ])
            .output();
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                if s.contains("Allow") {
                    PermState::Granted
                } else if s.contains("Deny") {
                    PermState::Denied
                } else {
                    PermState::Unknown
                }
            }
            Err(_) => PermState::Unknown,
        }
    }
}
