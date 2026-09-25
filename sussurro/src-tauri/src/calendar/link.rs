//! The private ICS link (#252, P22): Google's "secret address in iCal
//! format", Outlook's published calendar link, an iCloud public calendar.
//!
//! **The link is a secret** — anyone who has it reads the calendar. It is
//! kept only in the OS credential store (the #159 store, account
//! [`ACCOUNT`]), never in `settings.json`, a config export, diagnostics or a
//! log line. Where no credential store works it is simply not saved (there
//! is no clear-text fallback): the user can import the `.ics` file instead.
//! Errors and statuses name the host at most.
//!
//! **Fetching** goes through the URL source's shared client
//! ([`crate::sources::url::direct::fetch_bytes`]): http/https only, no
//! credentials in the link, hosts on this computer or the local network
//! refused (checked on every redirect, connections pinned to the checked
//! addresses, no system proxy), the model downloads' timeouts, and at most
//! [`super::MAX_ICS_BYTES`]. `webcal://` links (Apple, Outlook) are read as
//! `https://`.

use super::MAX_ICS_BYTES;
use crate::secrets::{SecretStore, StoreError};
use crate::sources::url::{self, direct, LocalAddressRefused};
use anyhow::{anyhow, bail, Result};
use reqwest::Url;
use serde::Serialize;

/// Credential-store account of the link (service [`crate::secrets::SERVICE`]).
pub const ACCOUNT: &str = "calendar:ics-link";

/// What Settings → Calendar shows. Never the link itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkStatus {
    /// A link is saved.
    pub saved: bool,
    /// Its host (`calendar.google.com`), to recognise it.
    pub host: Option<String>,
    /// A link can be saved (the credential store answers).
    pub store_available: bool,
    /// The store's name on this OS, for messages.
    pub store_name: &'static str,
    /// Why the saved link can't be read, or the store is unavailable.
    pub error: String,
}

/// Validate what the user pasted: `webcal(s)://` becomes `https://`, then
/// the URL source's rules (http/https, a host, no user name or password)
/// and no visibly local host. DNS is checked when the link is fetched.
pub fn normalize(input: &str) -> Result<Url> {
    let s = input.trim();
    if s.is_empty() {
        bail!("paste the calendar's private iCal (ICS) address");
    }
    let lower = s.to_ascii_lowercase();
    let s = if lower.starts_with("webcals://") || lower.starts_with("webcal://") {
        let rest = &s[s.find("://").map_or(0, |i| i + 3)..];
        format!("https://{rest}")
    } else {
        s.to_string()
    };
    let parsed = url::validate_url(&s).map_err(|e| {
        // The URL source's wording talks about media links.
        let msg = e.to_string();
        if msg.contains("start with https://") {
            anyhow!("not a valid link — a calendar link starts with https:// or webcal://")
        } else {
            e
        }
    })?;
    if url::is_visibly_local(&parsed) {
        bail!(local_message(parsed.host_str().unwrap_or_default()));
    }
    Ok(parsed)
}

fn local_message(host: &str) -> String {
    format!(
        "{host} is on this computer or the local network, and Sussurro doesn't fetch calendar \
         links there — export the calendar as an .ics file and import that instead"
    )
}

fn store_error(e: StoreError) -> anyhow::Error {
    anyhow!(
        "the link could not be saved in {} ({e}); import the calendar as an .ics file instead",
        crate::secrets::store_name()
    )
}

/// Save the link (validated) in the credential store, verified by reading
/// it back. Nothing is written anywhere else.
pub fn save(store: &dyn SecretStore, input: &str) -> Result<LinkStatus> {
    let url = normalize(input)?;
    store.set(ACCOUNT, url.as_str()).map_err(store_error)?;
    match store.get(ACCOUNT).map_err(store_error)? {
        Some(back) if back == url.as_str() => Ok(status(store)),
        _ => {
            let _ = store.delete(ACCOUNT);
            Err(store_error(StoreError(
                "the link read back does not match".into(),
            )))
        }
    }
}

/// Remove the saved link (no link saved is fine).
pub fn remove(store: &dyn SecretStore) -> Result<LinkStatus> {
    store.delete(ACCOUNT).map_err(|e| {
        anyhow!(
            "the link could not be removed from {} ({e})",
            crate::secrets::store_name()
        )
    })?;
    Ok(status(store))
}

/// The saved link, if any.
pub fn load(store: &dyn SecretStore) -> Result<Option<Url>> {
    let saved = store.get(ACCOUNT).map_err(|e| {
        anyhow!(
            "the calendar link could not be read from {} ({e})",
            crate::secrets::store_name()
        )
    })?;
    // Re-validated: the entry could have been edited by hand.
    saved.map(|s| normalize(&s)).transpose()
}

pub fn status(store: &dyn SecretStore) -> LinkStatus {
    let name = crate::secrets::store_name();
    match load(store) {
        Ok(link) => LinkStatus {
            saved: link.is_some(),
            host: link.and_then(|u| u.host_str().map(str::to_string)),
            store_available: true,
            store_name: name,
            error: String::new(),
        },
        Err(e) => LinkStatus {
            saved: false,
            host: None,
            store_available: crate::secrets::probe(store).is_ok(),
            store_name: name,
            error: e.to_string(),
        },
    }
}

/// Download the calendar behind `url` (see the module docs for the rules)
/// as text. `allow_local` is only for the tests' local server.
pub fn fetch(url: &Url, allow_local: bool) -> Result<String> {
    let host = url.host_str().unwrap_or_default().to_string();
    let fetched = direct::fetch_bytes(
        url,
        allow_local,
        "text/calendar, text/plain;q=0.5, */*;q=0.1",
        MAX_ICS_BYTES,
    )
    .map_err(|e| match e.downcast_ref::<LocalAddressRefused>() {
        Some(r) => anyhow!(local_message(&r.host)),
        None => e,
    })?;
    let text = String::from_utf8_lossy(&fetched.bytes).into_owned();
    let head = text.trim_start_matches('\u{feff}').trim_start();
    if !head
        .get(..15)
        .is_some_and(|h| h.eq_ignore_ascii_case("BEGIN:VCALENDAR"))
    {
        let html = fetched
            .content_type
            .as_deref()
            .is_some_and(|t| t.starts_with("text/html"))
            || head.starts_with('<');
        if html {
            bail!(
                "{host} answered with a web page, not a calendar — use the calendar's private \
                 iCal (ICS) address, not the page you see it on"
            );
        }
        bail!("{host} didn't send a calendar file");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::tests::FakeStore;
    use crate::sources::url::direct::tests::serve;

    const GOOGLE: &str = "https://calendar.google.com/calendar/ical/anna%40example.com/private-0123456789abcdef/basic.ics";

    #[test]
    fn links_are_validated_and_webcal_becomes_https() {
        assert_eq!(normalize(&format!("  {GOOGLE} ")).unwrap().as_str(), GOOGLE);
        assert_eq!(
            normalize("webcal://p01-caldav.icloud.com/published/2/abc")
                .unwrap()
                .as_str(),
            "https://p01-caldav.icloud.com/published/2/abc"
        );
        assert_eq!(
            normalize("WEBCALS://outlook.office365.com/owa/calendar/x/reachcalendar.ics")
                .unwrap()
                .scheme(),
            "https"
        );
        for (bad, want) in [
            ("", "paste"),
            ("calendar", "calendar link starts with"),
            ("ftp://example.com/cal.ics", "only http and https"),
            ("file:///etc/passwd", "only http and https"),
            (
                "https://user:pw@example.com/cal.ics",
                "user name or password",
            ),
            ("http://localhost:8080/cal.ics", "local network"),
            ("http://127.0.0.1/cal.ics", "local network"),
            ("http://169.254.169.254/latest/meta-data", "local network"),
            ("http://[::1]/cal.ics", "local network"),
            ("http://192.168.1.10/nextcloud/cal.ics", "local network"),
        ] {
            let e = normalize(bad).unwrap_err().to_string();
            assert!(e.contains(want), "{bad}: {e}");
        }
    }

    /// The link lives only in the credential store, and statuses never
    /// carry it.
    #[test]
    fn the_link_is_kept_in_the_store_only() {
        let store = FakeStore::default();
        let st = status(&store);
        assert!(!st.saved && st.store_available);

        let st = save(&store, GOOGLE).unwrap();
        assert!(st.saved);
        assert_eq!(st.host.as_deref(), Some("calendar.google.com"));
        assert_eq!(
            store.entries.borrow().get(ACCOUNT).map(String::as_str),
            Some(GOOGLE)
        );
        assert_eq!(load(&store).unwrap().unwrap().as_str(), GOOGLE);
        let json = serde_json::to_string(&st).unwrap();
        assert!(!json.contains("private-0123456789abcdef"), "{json}");

        // Replaced by a new one, then removed.
        save(&store, "webcal://example.com/other.ics").unwrap();
        assert_eq!(
            load(&store).unwrap().unwrap().as_str(),
            "https://example.com/other.ics"
        );
        let st = remove(&store).unwrap();
        assert!(!st.saved);
        assert!(store.entries.borrow().is_empty());
        // Removing twice is fine.
        remove(&store).unwrap();

        // An invalid link is never stored.
        assert!(save(&store, "http://localhost/cal.ics").is_err());
        assert_eq!(store.writes.get(), 2);
    }

    /// No credential store: the link is not saved at all (no clear-text
    /// fallback), and the error never repeats it.
    #[test]
    fn without_a_store_nothing_is_saved() {
        let store = FakeStore::default();
        store.broken.set(true);
        let e = save(&store, GOOGLE).unwrap_err().to_string();
        assert!(e.contains(".ics file"), "{e}");
        assert!(!e.contains("private-"), "{e}");
        let st = status(&store);
        assert!(!st.saved && !st.store_available);

        // A locked store: the saved link can't be read this session.
        let store = FakeStore::default();
        save(&store, GOOGLE).unwrap();
        store.locked.set(true);
        let st = status(&store);
        assert!(!st.saved);
        assert!(
            !st.error.is_empty() && !st.error.contains("private-"),
            "{}",
            st.error
        );
        assert!(load(&store).is_err());
    }

    /// A write that doesn't read back is removed again.
    #[test]
    fn an_unverified_write_is_undone() {
        struct Lossy(FakeStore);
        impl SecretStore for Lossy {
            fn get(&self, a: &str) -> Result<Option<String>, StoreError> {
                Ok(self.0.get(a)?.map(|s| s + "x"))
            }
            fn set(&self, a: &str, s: &str) -> Result<(), StoreError> {
                self.0.set(a, s)
            }
            fn delete(&self, a: &str) -> Result<(), StoreError> {
                self.0.delete(a)
            }
        }
        let store = Lossy(FakeStore::default());
        assert!(save(&store, GOOGLE).is_err());
        assert!(store.0.entries.borrow().is_empty());
    }

    const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";

    /// The shared client's rules apply to calendar links: a local server is
    /// refused (with the calendar's wording), redirects are checked, web
    /// pages and oversized bodies are refused, and errors never carry the
    /// link's path.
    #[test]
    fn fetching_follows_the_shared_network_rules() {
        let big = format!(
            "BEGIN:VCALENDAR\r\nX-PAD:{}\r\nEND:VCALENDAR\r\n",
            "a".repeat(64)
        );
        let srv = serve(vec![
            (
                "/private-secret/basic.ics",
                (
                    200,
                    vec![("Content-Type", "text/calendar".into())],
                    ICS.as_bytes().to_vec(),
                ),
            ),
            (
                "/go",
                (
                    302,
                    vec![("Location", "/private-secret/basic.ics".into())],
                    vec![],
                ),
            ),
            (
                "/page",
                (
                    200,
                    vec![("Content-Type", "text/html".into())],
                    b"<html>sign in</html>".to_vec(),
                ),
            ),
            (
                "/json",
                (
                    200,
                    vec![("Content-Type", "application/json".into())],
                    b"{}".to_vec(),
                ),
            ),
            (
                "/ftp",
                (
                    302,
                    vec![("Location", "ftp://example.com/x.ics".into())],
                    vec![],
                ),
            ),
            ("/gone", (404, vec![], vec![])),
            ("/big", (200, vec![], big.into_bytes())),
        ]);
        let u = |p: &str| Url::parse(&format!("{}{p}", srv.base)).unwrap();

        // Refused by the address rules: 127.0.0.1 is this computer.
        let e = fetch(&u("/private-secret/basic.ics"), false)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("local network") && e.contains(".ics file"),
            "{e}"
        );
        assert!(!e.contains("private-secret"), "{e}");

        // With the tests' exception, the redirect is followed.
        assert_eq!(fetch(&u("/go"), true).unwrap(), ICS);
        let err = |p: &str| format!("{:#}", fetch(&u(p), true).unwrap_err());
        assert!(err("/page").contains("web page"));
        assert!(err("/json").contains("didn't send a calendar"));
        assert!(err("/ftp").contains("only http and https"));
        let e = err("/gone");
        assert!(e.contains("404") && !e.contains("/gone"), "{e}");
    }

    #[test]
    fn oversized_calendars_are_refused() {
        let body = format!(
            "BEGIN:VCALENDAR\r\nX:{}\r\nEND:VCALENDAR\r\n",
            "a".repeat(2048)
        );
        let srv = serve(vec![("/cal.ics", (200, vec![], body.into_bytes()))]);
        let url = Url::parse(&format!("{}/cal.ics", srv.base)).unwrap();
        let e = direct::fetch_bytes(&url, true, "*/*", 1024)
            .unwrap_err()
            .to_string();
        assert!(e.contains("more than"), "{e}");
        assert!(direct::fetch_bytes(&url, true, "*/*", 1024 * 1024).is_ok());
    }

    /// A connection failure names the host, never the secret path.
    #[test]
    fn connection_errors_do_not_leak_the_link() {
        // A port nothing listens on.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let url = Url::parse(&format!("http://127.0.0.1:{port}/private-secret/basic.ics")).unwrap();
        let e = format!("{:#}", fetch(&url, true).unwrap_err());
        assert!(e.contains("127.0.0.1"), "{e}");
        assert!(!e.contains("private-secret"), "{e}");
    }

    /// A real feed: `SUSSURRO_LIVE_ICS=<link> cargo test live_ics -- --ignored`.
    #[test]
    #[ignore]
    fn live_ics_link() {
        let Ok(link) = std::env::var("SUSSURRO_LIVE_ICS") else {
            return;
        };
        let text = fetch(&normalize(&link).unwrap(), false).unwrap();
        let cal =
            super::super::IcsCalendar::parse(&text, chrono::FixedOffset::east_opt(0).unwrap())
                .unwrap();
        assert!(!cal.is_empty());
    }
}
