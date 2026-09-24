//! macOS version gating for Core Audio process taps (#140): the app's
//! minimum stays 11.0, and the native choice is offered from 14.2 on. Pure,
//! so it is tested on every OS.

/// (major, minor, patch).
pub type OsVersion = (u64, u64, u64);

/// `AudioHardwareCreateProcessTap` and `CATapDescription` arrived in 14.2.
pub const PROCESS_TAP_MIN: OsVersion = (14, 2, 0);

pub fn supports_process_tap(v: OsVersion) -> bool {
    v >= PROCESS_TAP_MIN
}

/// "14.2.1", "15.0", "27" (as `sw_vers -productVersion` prints it) → the
/// version; missing parts are 0.
pub fn parse(s: &str) -> Option<OsVersion> {
    let mut parts = s.trim().split('.');
    let major = parts.next()?.trim().parse().ok()?;
    let minor = match parts.next() {
        Some(p) => p.trim().parse().ok()?,
        None => 0,
    };
    let patch = match parts.next() {
        Some(p) => p.trim().parse().ok()?,
        None => 0,
    };
    Some((major, minor, patch))
}

/// "13.6.1", or "13.6" when the patch is 0.
pub fn display(v: OsVersion) -> String {
    if v.2 == 0 {
        format!("{}.{}", v.0, v.1)
    } else {
        format!("{}.{}.{}", v.0, v.1, v.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_taps_need_14_2() {
        for (v, ok) in [
            ((11, 0, 0), false),
            ((13, 6, 1), false),
            ((14, 0, 0), false),
            ((14, 1, 2), false),
            ((14, 2, 0), true),
            ((14, 2, 1), true),
            ((14, 10, 0), true),
            ((15, 0, 0), true),
            ((26, 0, 0), true),
            ((27, 0, 0), true),
        ] {
            assert_eq!(supports_process_tap(v), ok, "{v:?}");
        }
    }

    #[test]
    fn versions_parse_like_sw_vers_prints_them() {
        assert_eq!(parse("14.2.1"), Some((14, 2, 1)));
        assert_eq!(parse("15.0\n"), Some((15, 0, 0)));
        assert_eq!(parse("27"), Some((27, 0, 0)));
        assert_eq!(parse(""), None);
        assert_eq!(parse("fourteen"), None);
        assert_eq!(parse("14.x"), None);
        assert_eq!(display((13, 6, 1)), "13.6.1");
        assert_eq!(display((14, 2, 0)), "14.2");
    }
}
