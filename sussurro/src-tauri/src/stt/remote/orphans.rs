//! Sidecars orphaned by a crash of Sussurro (#319).
//!
//! On Windows (a kill-on-close job object) and Linux (`PR_SET_PDEATHSIG`)
//! the OS stops a sidecar when Sussurro dies, however it dies. macOS has
//! neither: after a crash or a force-quit, launchd re-parents the
//! `llama-server` child (ppid 1) and it keeps its model in memory (~2 GB)
//! with nobody able to reach it — the next start's folder sweep
//! ([`super::endpoint::sweep_stale_run_dirs`]) removes its socket folder,
//! not the process.
//!
//! So on macOS, at startup and before the first spawn of each sidecar
//! executable, [`reap_orphans`] stops the processes that pass **every**
//! check of [`select_orphans`]:
//! - owned by the current user;
//! - parent pid 1 (orphaned);
//! - running our sidecar executable — the same canonical path as the one
//!   the app spawns, never a match by name (a user's own `llama-server`
//!   runs from somewhere else);
//! - its `--host` argument is `<…>/sc-<pid>-<tag>/llama.sock`, the socket in
//!   one of our per-run folders, and that Sussurro `<pid>` is dead.
//!
//! Each gets SIGTERM, then SIGKILL if it is still there after a short grace
//! — re-checked against the same rules right before the SIGKILL, so a pid
//! reused in between is never hit. The selection is pure (a process list →
//! the pids to stop) and compiled and tested on every platform; only the
//! enumeration and the signals are macOS code.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// What the selection needs to know about one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    /// The executable's canonical path, when it could be read.
    pub exe: Option<PathBuf>,
    /// The command line, `argv[0]` first.
    pub args: Vec<OsString>,
}

/// The Sussurro pid named by a sidecar's `--host` argument, when it is the
/// socket in one of our per-run folders (`…/sc-<pid>-<tag>/llama.sock`).
/// Only the separate `--host <value>` form, which is the one we pass. Pure.
fn host_run_pid(args: &[OsString]) -> Option<u32> {
    let i = args.iter().position(|a| a == "--host")?;
    let host = Path::new(args.get(i + 1)?);
    if host.file_name()? != super::endpoint::SOCKET_NAME {
        return None;
    }
    let dir = host.parent()?.file_name()?.to_str()?;
    super::endpoint::run_dir_pid(dir)
}

/// The pids of orphaned sidecars to stop: owned by `my_uid`, parent pid 1,
/// running `sidecar` (already canonical, compared with each process's
/// canonical executable path) and listening in a per-run folder of a
/// Sussurro pid that `alive` says is gone. Never `current_pid`. Pure.
pub fn select_orphans(
    procs: &[ProcInfo],
    my_uid: u32,
    current_pid: u32,
    sidecar: &Path,
    alive: impl Fn(u32) -> bool,
) -> Vec<u32> {
    procs
        .iter()
        .filter(|p| {
            p.pid != current_pid
                && p.pid > 1
                && p.uid == my_uid
                && p.ppid == 1
                && p.exe.as_deref() == Some(sidecar)
                && host_run_pid(&p.args)
                    .is_some_and(|owner| owner != current_pid && owner != p.pid && !alive(owner))
        })
        .map(|p| p.pid)
        .collect()
}

/// How long an orphan gets to exit after SIGTERM before SIGKILL.
pub const GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Stop the orphaned sidecars running `sidecar` (macOS; see the module
/// docs). Runs once per executable path per process; later calls return 0.
/// Returns how many processes were signalled.
#[cfg(target_os = "macos")]
pub fn reap_orphans(sidecar: &Path) -> usize {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static DONE: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let Ok(sidecar) = std::fs::canonicalize(sidecar) else {
        return 0;
    };
    // Held for the whole reap: a concurrent first spawn waits for it.
    let mut done = DONE
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if !done.insert(sidecar.clone()) {
        return 0;
    }
    let n = reap_with_grace(&sidecar, GRACE);
    if n > 0 {
        eprintln!("stopped {n} orphaned llama-server sidecar(s) left by an earlier run");
    }
    n
}

/// No orphans to look for: the OS already stops the sidecar with the app.
#[cfg(not(target_os = "macos"))]
pub fn reap_orphans(_sidecar: &Path) -> usize {
    0
}

/// The reap itself, without the once-per-path guard (`sidecar` canonical).
#[cfg(target_os = "macos")]
pub(crate) fn reap_with_grace(sidecar: &Path, grace: std::time::Duration) -> usize {
    use crate::engine::checkpoint::process_alive;
    use std::time::Instant;
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    let current = std::process::id();
    let select = |procs: &[ProcInfo]| select_orphans(procs, me, current, sidecar, process_alive);
    let targets = select(&macos::processes());
    for &pid in &targets {
        // SAFETY: plain signal to a pid that passed every check.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
    let deadline = Instant::now() + grace;
    let mut left: Vec<u32> = targets.clone();
    while !left.is_empty() && Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
        left.retain(|&pid| process_alive(pid));
    }
    if !left.is_empty() {
        // Still the same orphan (not a pid reused meanwhile)?
        let again = select(&macos::processes_of(&left));
        for pid in again {
            // SAFETY: as above, re-checked just now.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        }
    }
    targets.len()
}

#[cfg(target_os = "macos")]
mod macos {
    //! Process enumeration through libproc and `KERN_PROCARGS2` (libc only).

    use super::ProcInfo;
    use std::ffi::{c_void, OsString};
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    /// Every process we can read (others are skipped).
    pub fn processes() -> Vec<ProcInfo> {
        processes_of(&all_pids())
    }

    /// The readable processes among `pids`.
    pub fn processes_of(pids: &[u32]) -> Vec<ProcInfo> {
        pids.iter().filter_map(|&pid| info(pid)).collect()
    }

    fn all_pids() -> Vec<u32> {
        // SAFETY: a null buffer asks for the number of pids.
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return Vec::new();
        }
        // Room for processes started meanwhile.
        let mut buf: Vec<libc::pid_t> = vec![0; n as usize + 64];
        let bytes = (buf.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: `buf` is writable for `bytes` bytes.
        let got = unsafe { libc::proc_listallpids(buf.as_mut_ptr().cast::<c_void>(), bytes) };
        if got <= 0 {
            return Vec::new();
        }
        buf.truncate((got as usize).min(buf.len()));
        buf.into_iter()
            .filter(|&p| p > 0)
            .map(|p| p as u32)
            .collect()
    }

    fn info(pid: u32) -> Option<ProcInfo> {
        let ipid = libc::c_int::try_from(pid).ok()?;
        // SAFETY: zeroed is a valid proc_bsdinfo (plain integers and arrays).
        let mut bsd: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: `bsd` is writable for `size` bytes.
        let got = unsafe {
            libc::proc_pidinfo(
                ipid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut bsd as *mut libc::proc_bsdinfo).cast::<c_void>(),
                size,
            )
        };
        if got != size {
            return None;
        }
        Some(ProcInfo {
            pid,
            ppid: bsd.pbi_ppid,
            uid: bsd.pbi_uid,
            exe: exe_path(ipid),
            args: args(ipid).unwrap_or_default(),
        })
    }

    fn exe_path(pid: libc::c_int) -> Option<PathBuf> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: `buf` is writable for its length.
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        let path = PathBuf::from(OsString::from_vec(buf));
        // Compared with the sidecar's canonical path.
        std::fs::canonicalize(&path).ok()
    }

    /// `argv` from `KERN_PROCARGS2`: `argc` (i32), the exec path, NUL
    /// padding, then `argc` NUL-terminated arguments (then the env, unread).
    fn args(pid: libc::c_int) -> Option<Vec<OsString>> {
        let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
        let mut argmax: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>();
        // SAFETY: `argmax` is writable for `len` bytes.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                2,
                (&mut argmax as *mut libc::c_int).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 || argmax <= 0 {
            return None;
        }
        let mut buf = vec![0u8; argmax as usize];
        let mut len = buf.len();
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        // SAFETY: `buf` is writable for `len` bytes; the call sets `len`.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return None;
        }
        parse_procargs2(&buf[..len.min(buf.len())])
    }

    /// Parse a `KERN_PROCARGS2` buffer. Pure.
    pub(super) fn parse_procargs2(buf: &[u8]) -> Option<Vec<OsString>> {
        let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
        let argc = usize::try_from(argc).ok()?;
        let mut rest = &buf[4..];
        // The exec path, then its NUL padding.
        let end = rest.iter().position(|&b| b == 0)?;
        rest = &rest[end..];
        let start = rest.iter().position(|&b| b != 0)?;
        rest = &rest[start..];
        let mut out = Vec::with_capacity(argc);
        for _ in 0..argc {
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            out.push(OsString::from_vec(rest[..end].to_vec()));
            rest = rest.get(end + 1..).unwrap_or(&[]);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 501;
    const CURRENT: u32 = 4000;
    const DEAD: u32 = 3000;
    const LIVE_PARENT: u32 = 3500;

    fn sidecar() -> PathBuf {
        PathBuf::from("/Applications/Sussurro.app/Contents/MacOS/sussurro-llama-server")
    }

    fn host(owner: u32) -> String {
        format!("/Users/u/Library/Application Support/x/sidecar/sc-{owner}-ab12cd34/llama.sock")
    }

    fn proc(pid: u32, host: Option<&str>) -> ProcInfo {
        let mut args: Vec<OsString> = vec![sidecar().into(), "-m".into(), "/m/model.gguf".into()];
        if let Some(h) = host {
            args.push("--host".into());
            args.push(h.into());
        }
        args.push("--no-webui".into());
        ProcInfo {
            pid,
            ppid: 1,
            uid: ME,
            exe: Some(sidecar()),
            args,
        }
    }

    fn select(procs: &[ProcInfo]) -> Vec<u32> {
        select_orphans(procs, ME, CURRENT, &sidecar(), |pid| {
            pid == LIVE_PARENT || pid == CURRENT
        })
    }

    #[test]
    fn an_orphan_of_a_dead_run_is_selected() {
        assert_eq!(select(&[proc(100, Some(&host(DEAD)))]), vec![100]);
    }

    #[test]
    fn anything_failing_a_check_is_left_alone() {
        let orphan = proc(100, Some(&host(DEAD)));

        // A user's own llama-server, elsewhere on disk, same arguments.
        let mut foreign = orphan.clone();
        foreign.pid = 101;
        foreign.exe = Some("/opt/homebrew/bin/llama-server".into());
        // Same file name in another folder: never matched by name.
        let mut same_name = orphan.clone();
        same_name.pid = 102;
        same_name.exe = Some("/tmp/other/sussurro-llama-server".into());
        // Unknown executable.
        let mut no_exe = orphan.clone();
        no_exe.pid = 103;
        no_exe.exe = None;
        // Its Sussurro is still running (and it is somehow re-parented).
        let live = proc(104, Some(&host(LIVE_PARENT)));
        // The running app's own folder.
        let ours = proc(105, Some(&host(CURRENT)));
        // Another user's process.
        let mut other_user = orphan.clone();
        other_user.pid = 106;
        other_user.uid = 502;
        // Not orphaned: a live app's child.
        let mut child = orphan.clone();
        child.pid = 107;
        child.ppid = LIVE_PARENT;
        // No --host at all (a TCP server, or not ours).
        let no_host = proc(108, None);
        // --host not naming one of our run folders.
        let tcp = proc(109, Some("127.0.0.1"));
        let not_sc = proc(110, Some("/tmp/mine/llama.sock"));
        let other_file = proc(111, Some(&host(DEAD).replace("llama.sock", "x.sock")));
        let bad_pid = proc(112, Some("/tmp/sc-abc-1234/llama.sock"));
        // `--host=` form is not the one we pass.
        let mut eq_form = orphan.clone();
        eq_form.pid = 113;
        eq_form.args = vec![sidecar().into(), format!("--host={}", host(DEAD)).into()];
        // The flag as the last argument, with no value.
        let mut dangling = orphan.clone();
        dangling.pid = 114;
        dangling.args = vec![sidecar().into(), "--host".into()];
        // Never ourselves, never launchd.
        let mut me = orphan.clone();
        me.pid = CURRENT;
        let mut init = orphan.clone();
        init.pid = 1;

        let all = [
            foreign, same_name, no_exe, live, ours, other_user, child, no_host, tcp, not_sc,
            other_file, bad_pid, eq_form, dangling, me, init,
        ];
        assert!(select(&all).is_empty(), "{:?}", select(&all));

        // The real orphan among them is still found.
        let mut with_orphan = all.to_vec();
        with_orphan.push(orphan);
        assert_eq!(select(&with_orphan), vec![100]);
    }

    #[test]
    fn a_host_names_the_run_pid() {
        let args = |h: &str| vec![OsString::from("x"), "--host".into(), h.into()];
        assert_eq!(host_run_pid(&args(&host(42))), Some(42));
        assert_eq!(host_run_pid(&args("/tmp/sc-7-x/llama.sock")), Some(7));
        assert_eq!(host_run_pid(&args("sc-7-x")), None);
        assert_eq!(host_run_pid(&args("llama.sock")), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn procargs2_buffers_parse() {
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/bin/exe\0\0\0\0/bin/exe\0--host\0/t/sc-1-a/llama.sock\0HOME=/x\0");
        let args = macos::parse_procargs2(&buf).unwrap();
        assert_eq!(args, ["/bin/exe", "--host", "/t/sc-1-a/llama.sock"]);
        assert_eq!(macos::parse_procargs2(&[1, 0]), None);
    }

    /// Our own process reads back with its uid, parent and argv, and is never
    /// an orphan to stop (enumeration only — nothing is signalled).
    #[cfg(target_os = "macos")]
    #[test]
    fn this_process_is_enumerated_and_not_selected() {
        let me = std::process::id();
        let procs = macos::processes();
        let mine = procs
            .iter()
            .find(|p| p.pid == me)
            .expect("own process listed");
        // SAFETY: geteuid/getppid have no preconditions.
        assert_eq!(mine.uid, unsafe { libc::geteuid() });
        assert_eq!(mine.ppid, unsafe { libc::getppid() } as u32);
        assert_eq!(
            mine.exe.as_deref(),
            Some(
                std::fs::canonicalize(std::env::current_exe().unwrap())
                    .unwrap()
                    .as_path()
            )
        );
        assert!(!mine.args.is_empty());
        let exe = mine.exe.clone().unwrap();
        let uid = mine.uid;
        assert!(select_orphans(
            &procs,
            uid,
            me,
            &exe,
            crate::engine::checkpoint::process_alive
        )
        .is_empty());
    }

    /// The fake sidecar of [`live_an_orphaned_sidecar_is_reaped`]: a copy of
    /// this test binary started with `--exact <this test> -- --host …`
    /// idles until it is stopped (at most 60 s). In an ordinary test run
    /// there is no `--host` and it returns at once.
    #[test]
    fn fake_orphan_sidecar() {
        if !std::env::args().any(|a| a == "--host") {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(60));
        std::process::exit(0);
    }

    /// #319 end to end: a fake sidecar (a copy of this test binary)
    /// orphaned to launchd with a `--host` in the folder of a dead pid is
    /// stopped; a twin whose run pid is alive is not. Spawns processes, so
    /// ignored: `cargo test orphans::tests::live_ -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn live_an_orphaned_sidecar_is_reaped() {
        use crate::engine::checkpoint::process_alive;
        use std::process::Command;
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("sussurro-llama-server");
        // A clone on APFS: instant, keeps the linker's ad-hoc signature.
        std::fs::copy(std::env::current_exe().unwrap(), &fake).unwrap();
        let fake = std::fs::canonicalize(&fake).unwrap();

        // A pid that is certainly dead: a child we already waited for.
        let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
        gone.wait().unwrap();
        let dead = gone.id();
        assert!(!process_alive(dead));
        let me = std::process::id();

        let spawn_orphan = |owner: u32| -> u32 {
            let host = dir
                .path()
                .join(format!("sc-{owner}-deadbeef"))
                .join("llama.sock");
            // The intermediate shell starts the fake in the background and
            // exits at once, so launchd adopts it (ppid 1).
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(r#""$0" --exact stt::remote::orphans::tests::fake_orphan_sidecar --test-threads=1 -- --host "$1" </dev/null >/dev/null 2>&1 & echo $!"#)
                .arg(&fake)
                .arg(&host)
                .output()
                .unwrap();
            String::from_utf8(out.stdout)
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        };
        let orphan = spawn_orphan(dead);
        let kept = spawn_orphan(me);
        let cleanup = || {
            for pid in [orphan, kept] {
                // SAFETY: our own test processes.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            }
        };
        // Wait for the re-parenting and the exec.
        let ready = || {
            let ps = macos::processes_of(&[orphan, kept]);
            ps.len() == 2
                && ps.iter().all(|p| {
                    p.ppid == 1
                        && p.exe.as_deref() == Some(fake.as_path())
                        && p.args.iter().any(|a| a == "--host")
                })
        };
        let t0 = Instant::now();
        while !ready() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if !ready() {
            let ps = macos::processes_of(&[orphan, kept]);
            cleanup();
            panic!("the fakes are not orphans of the fake sidecar: {ps:?}");
        }

        let n = reap_with_grace(&fake, Duration::from_secs(2));
        let t0 = Instant::now();
        while process_alive(orphan) && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let orphan_alive = process_alive(orphan);
        let kept_alive = process_alive(kept);
        cleanup();
        assert_eq!(n, 1);
        assert!(!orphan_alive, "the orphan of a dead run is gone");
        assert!(kept_alive, "the one whose run pid is alive is left alone");
    }
}
