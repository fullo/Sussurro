//! Where a sidecar listens and why its answers can be trusted (#216).
//!
//! A loopback TCP port is not private: any local user can bind it. Picking
//! a free port, dropping it and then spawning `llama-server` leaves a
//! window in which another user's process can take the port, pass the
//! `/health` check and receive the audio (or the text to clean) and return
//! forged transcripts. So:
//!
//! - **macOS and Linux** — the server listens on a Unix socket
//!   (`--host <dir>/llama.sock`, supported by the pinned b11146) inside a
//!   fresh per-run folder created with mode 0700 ([`private_run_dir`]):
//!   no other user can connect to it or put a socket of their own there.
//!   The folder is under app data (`<app data>/sidecar/`), or the temporary
//!   folder when that path would be too long for a socket address; it is
//!   removed when the process stops, and folders of dead processes are
//!   swept when a new one is created.
//! - **Windows** — loopback TCP on a random port, and after `/health`
//!   answers, the listening socket must belong to the child: the owning
//!   PID of every `127.0.0.1:<port>` listener, from the IP Helper API
//!   (`GetExtendedTcpTable`, no external tools), must be the child's
//!   ([`check_listener_owner`]).
//!
//! On every platform each spawn gets a random API key ([`new_api_key`]),
//! passed in the environment (`LLAMA_API_KEY` — never on the command line,
//! where other users could read it) and sent as `Authorization: Bearer` on
//! every `/v1` request: a web page or another local program can't use the
//! server even where it can reach it. (It does not authenticate the server
//! to us — `/health` is public — which is what the socket/owner checks do.)

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Where a running sidecar listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A Unix socket in a private (0700) per-run folder (macOS, Linux).
    Unix(PathBuf),
    /// A loopback TCP port whose listener was checked to be the child
    /// (Windows).
    Tcp(u16),
}

impl Endpoint {
    /// The base URL requests use. On a Unix socket the host is only the
    /// `Host` header (`localhost`); the socket path is the address.
    pub fn base_url(&self) -> String {
        match self {
            Endpoint::Unix(_) => "http://localhost".to_string(),
            Endpoint::Tcp(port) => format!("http://127.0.0.1:{port}"),
        }
    }
}

/// How to reach a running sidecar: its endpoint and this spawn's API key.
#[derive(Clone, PartialEq, Eq)]
pub struct SidecarAccess {
    pub(super) endpoint: Endpoint,
    pub(super) api_key: String,
}

impl std::fmt::Debug for SidecarAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SidecarAccess")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl SidecarAccess {
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// See [`Endpoint::base_url`].
    pub fn base_url(&self) -> String {
        self.endpoint.base_url()
    }

    /// The key every `/v1` request must carry as a bearer token.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// A client builder for this sidecar: never through a proxy, and on the
    /// Unix socket where there is one. Callers add their timeouts.
    pub fn client_builder(&self) -> reqwest::blocking::ClientBuilder {
        let b = reqwest::blocking::Client::builder().no_proxy();
        match &self.endpoint {
            #[cfg(unix)]
            Endpoint::Unix(path) => b.unix_socket(path.as_path()),
            #[cfg(not(unix))]
            Endpoint::Unix(_) => b,
            Endpoint::Tcp(_) => b,
        }
    }
}

/// A fresh random API key: 32 bytes, hex.
pub fn new_api_key() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("no OS randomness: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

// ------------------------------------------------------- Unix socket dir --

/// Name prefix of the per-run folders: `sc-<pid>-<random>`.
const RUN_PREFIX: &str = "sc-";
/// The socket's file name (llama-server wants a `.sock` suffix).
pub const SOCKET_NAME: &str = "llama.sock";
/// Longest socket path used: `sun_path` holds 104 bytes on macOS, 108 on
/// Linux, including the terminating NUL.
pub const MAX_SOCKET_PATH: usize = 100;

/// `<base>/sc-<pid>-<tag>/llama.sock` fits a socket address. Pure.
pub fn socket_path_fits(base: &Path, pid: u32, tag: &str) -> bool {
    let p = base
        .join(format!("{RUN_PREFIX}{pid}-{tag}"))
        .join(SOCKET_NAME);
    p.as_os_str().len() <= MAX_SOCKET_PATH
}

/// `sc-<pid>-<tag>` → `pid`. Pure.
fn run_dir_pid(name: &str) -> Option<u32> {
    name.strip_prefix(RUN_PREFIX)?
        .split('-')
        .next()?
        .parse()
        .ok()
}

/// Remove the per-run folders of processes that are gone (a crash or a
/// force-quit of Sussurro). Returns how many were removed.
pub fn sweep_stale_run_dirs(base: &Path, current_pid: u32) -> usize {
    let Ok(entries) = std::fs::read_dir(base) else {
        return 0;
    };
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = run_dir_pid(&name) else {
            continue;
        };
        if pid == current_pid || crate::engine::checkpoint::process_alive(pid) {
            continue;
        }
        // Only our own real folders (never a link; in the shared temporary
        // folder, never another user's).
        if !e.file_type().is_ok_and(|t| t.is_dir()) || !owned_by_me(&e.path()) {
            continue;
        }
        if std::fs::remove_dir_all(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The app's folder for the per-run socket folders: `<app data>/sidecar`.
pub fn app_socket_base(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    app.path().app_data_dir().ok().map(|d| d.join("sidecar"))
}

#[cfg(unix)]
fn owned_by_me(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    std::fs::symlink_metadata(path).is_ok_and(|m| m.uid() == me)
}

#[cfg(not(unix))]
fn owned_by_me(_path: &Path) -> bool {
    true
}

/// Create a fresh private folder for one server's socket, under the first
/// of `bases` where the socket path fits: created with mode 0700 by this
/// call (never an existing one), checked to be a real folder owned by us
/// with no access for anyone else. Returns the folder and the socket path.
///
/// A base must be a folder other users can't rename entries in: the
/// user's app data, or the temporary folder itself (per user on macOS,
/// sticky `/tmp` on Linux) — never a subfolder of `/tmp` someone else
/// may have created first.
#[cfg(unix)]
pub fn private_run_dir(bases: &[PathBuf]) -> Result<(PathBuf, PathBuf)> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let pid = std::process::id();
    let mut last = anyhow!("no folder for the sidecar's socket");
    for base in bases {
        let tag_len = 8;
        if !socket_path_fits(base, pid, &"0".repeat(tag_len)) {
            last = anyhow!("{} is too long for a socket path", base.display());
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(base) {
            last = anyhow::Error::new(e).context(format!("creating {}", base.display()));
            continue;
        }
        sweep_stale_run_dirs(base, pid);
        for _ in 0..8 {
            let tag = &new_api_key()?[..tag_len];
            let dir = base.join(format!("{RUN_PREFIX}{pid}-{tag}"));
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    last = anyhow::Error::new(e).context(format!("creating {}", dir.display()));
                    break;
                }
            }
            let meta = std::fs::symlink_metadata(&dir)
                .with_context(|| format!("checking {}", dir.display()))?;
            // SAFETY: geteuid has no preconditions.
            let me = unsafe { libc::geteuid() };
            if !meta.is_dir() || meta.uid() != me || meta.permissions().mode() & 0o077 != 0 {
                let _ = std::fs::remove_dir(&dir);
                anyhow::bail!("{} is not a private folder", dir.display());
            }
            let sock = dir.join(SOCKET_NAME);
            return Ok((dir, sock));
        }
    }
    Err(last)
}

// ------------------------------------------------- Windows port ownership --

/// Whether `pid` owns every IPv4 listener on `127.0.0.1:port` in `rows`
/// (`(address, port, owning pid)`), and there is at least one. A listener
/// of another process on the same address — a squatter, or one sharing the
/// port — fails. Pure.
pub fn check_listener_owner(
    rows: &[(std::net::Ipv4Addr, u16, u32)],
    port: u16,
    pid: u32,
) -> std::result::Result<(), String> {
    let on_port: Vec<u32> = rows
        .iter()
        .filter(|(addr, p, _)| *p == port && addr.is_loopback())
        .map(|&(_, _, owner)| owner)
        .collect();
    if on_port.is_empty() {
        return Err(format!("nothing listens on 127.0.0.1:{port}"));
    }
    match on_port.iter().find(|&&owner| owner != pid) {
        Some(other) => Err(format!(
            "127.0.0.1:{port} is held by another process (pid {other}), not the sidecar (pid {pid})"
        )),
        None => Ok(()),
    }
}

/// Every IPv4 TCP listener: address, port and owning process id.
#[cfg(windows)]
pub fn tcp_listeners() -> std::io::Result<Vec<(std::net::Ipv4Addr, u16, u32)>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
        TCP_TABLE_OWNER_PID_LISTENER,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    let mut size: u32 = 0;
    for _ in 0..8 {
        // u64 elements: the table needs 4-byte alignment.
        let mut buf: Vec<u64> = vec![0; (size as usize).div_ceil(8).max(1)];
        let mut len = (buf.len() * 8) as u32;
        // SAFETY: `buf` is writable for `len` bytes; the call writes at most
        // that and reports the size it needs otherwise.
        let rc = unsafe {
            GetExtendedTcpTable(
                buf.as_mut_ptr().cast(),
                &mut len,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if rc == ERROR_INSUFFICIENT_BUFFER {
            size = len;
            continue;
        }
        if rc != 0 {
            return Err(std::io::Error::from_raw_os_error(rc as i32));
        }
        // SAFETY: on success the buffer holds a MIB_TCPTABLE_OWNER_PID with
        // `dwNumEntries` rows, within the `len` bytes written.
        let rows = unsafe {
            let table = &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
            std::slice::from_raw_parts(
                table.table.as_ptr() as *const MIB_TCPROW_OWNER_PID,
                table.dwNumEntries as usize,
            )
        };
        return Ok(rows
            .iter()
            .map(|r| {
                (
                    std::net::Ipv4Addr::from(r.dwLocalAddr.to_ne_bytes()),
                    u16::from_be((r.dwLocalPort & 0xffff) as u16),
                    r.dwOwningPid,
                )
            })
            .collect());
    }
    Err(std::io::Error::other("the TCP table kept growing"))
}

/// Fail unless the child `pid` owns the listener on `127.0.0.1:port`.
#[cfg(windows)]
pub fn verify_listener_owner(port: u16, pid: u32) -> Result<()> {
    let rows = tcp_listeners().context("reading the TCP listener table")?;
    check_listener_owner(&rows, port, pid).map_err(|e| anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn api_keys_are_random_and_long() {
        let a = new_api_key().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, new_api_key().unwrap());
        let access = SidecarAccess {
            endpoint: Endpoint::Tcp(1),
            api_key: a.clone(),
        };
        assert!(
            !format!("{access:?}").contains(&a),
            "the key is never printed"
        );
        assert_eq!(access.base_url(), "http://127.0.0.1:1");
        assert_eq!(
            Endpoint::Unix("/x/llama.sock".into()).base_url(),
            "http://localhost"
        );
    }

    #[test]
    fn the_listener_must_belong_to_the_child() {
        let lo = Ipv4Addr::LOCALHOST;
        let any = Ipv4Addr::UNSPECIFIED;
        // Ours alone (other ports and a wildcard listener don't matter).
        let rows = [(lo, 5000, 42), (lo, 6000, 7), (any, 5000, 7)];
        assert!(check_listener_owner(&rows, 5000, 42).is_ok());
        // A squatter got the port first: the child never listened.
        let e = check_listener_owner(&[(lo, 5000, 7)], 5000, 42).unwrap_err();
        assert!(e.contains("pid 7"), "{e}");
        // Sharing the port with another process is refused too.
        assert!(check_listener_owner(&[(lo, 5000, 42), (lo, 5000, 7)], 5000, 42).is_err());
        // Nobody listens.
        assert!(check_listener_owner(&rows, 5001, 42).is_err());
    }

    #[test]
    fn socket_paths_fit_the_address_limit() {
        assert!(socket_path_fits(Path::new("/tmp"), 123, "abcdef12"));
        let long = PathBuf::from(format!("/Users/{}/Library", "u".repeat(90)));
        assert!(!socket_path_fits(&long, 123, "abcdef12"));
        assert_eq!(run_dir_pid("sc-123-abcdef"), Some(123));
        assert_eq!(run_dir_pid("other"), None);
    }

    #[cfg(unix)]
    #[test]
    fn run_dirs_are_private_fresh_and_swept_after_a_crash() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let base = root.path().join("sidecar");
        let (dir, sock) = private_run_dir(std::slice::from_ref(&base)).unwrap();
        assert_eq!(sock, dir.join("llama.sock"));
        assert!(sock.as_os_str().len() <= MAX_SOCKET_PATH);
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
        // Every run gets its own folder.
        let (dir2, _) = private_run_dir(std::slice::from_ref(&base)).unwrap();
        assert_ne!(dir, dir2);

        // A base too long for a socket path: the next one is used.
        let long = root.path().join("x".repeat(120));
        let (dir3, sock3) = private_run_dir(&[long.clone(), base.clone()]).unwrap();
        assert!(dir3.starts_with(&base) && !long.exists());
        assert!(sock3.as_os_str().len() <= MAX_SOCKET_PATH);

        // A dead process's folder goes; ours and unrelated ones stay.
        let dead = base.join("sc-999999999-aaaaaaaa");
        std::fs::create_dir(&dead).unwrap();
        std::fs::write(dead.join("llama.sock"), b"").unwrap();
        let unrelated = base.join("notes");
        std::fs::create_dir(&unrelated).unwrap();
        assert_eq!(sweep_stale_run_dirs(&base, std::process::id()), 1);
        assert!(!dead.exists() && unrelated.exists() && dir.exists() && dir2.exists());
    }
}
