//! Cheap per-OS process readings (#101): memory of this process or of a
//! sidecar, and this process's CPU time — no extra crate. Every function
//! returns `None` rather than failing.
//!
//! Memory is what the OS task manager shows: macOS the physical footprint
//! (Activity Monitor's "Memory", which also counts Metal buffers), Linux
//! the resident set (`/proc/<pid>/statm`), Windows the working set.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Memory of this process, bytes.
pub fn self_memory() -> Option<u64> {
    memory_of(std::process::id())
}

/// Memory of process `pid` (one of ours: sidecars), bytes.
pub fn memory_of(pid: u32) -> Option<u64> {
    imp::memory_of(pid)
}

/// CPU time this process used so far (user + system, all threads).
pub fn self_cpu_time() -> Option<Duration> {
    imp::self_cpu_time()
}

/// Logical CPUs.
pub fn cpu_count() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// CPU load between two readings, as a percentage of the whole machine
/// (all cores = 100 %). Pure.
pub fn cpu_percent(cpu_delta: Duration, wall_delta: Duration, cpus: usize) -> Option<f64> {
    if wall_delta.is_zero() || cpus == 0 {
        return None;
    }
    let pct = cpu_delta.as_secs_f64() / wall_delta.as_secs_f64() / cpus as f64 * 100.0;
    Some(pct.clamp(0.0, 100.0))
}

/// Turns successive CPU-time readings into a load figure: each call
/// compares with the previous one (the panel polls once a second).
#[derive(Default)]
pub struct CpuMeter {
    last: Mutex<Option<(Instant, Duration)>>,
}

impl CpuMeter {
    pub const fn new() -> Self {
        Self {
            last: Mutex::new(None),
        }
    }

    /// Record a reading taken at `now`; the load since the previous one
    /// (`None` on the first call, or when readings are under 200 ms apart
    /// — too short to mean anything, the previous reading is kept).
    pub fn sample(&self, now: Instant, cpu: Duration, cpus: usize) -> Option<f64> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        match *last {
            Some((t, _)) if now.saturating_duration_since(t) < Duration::from_millis(200) => None,
            Some((t, c)) => {
                *last = Some((now, cpu));
                cpu_percent(
                    cpu.saturating_sub(c),
                    now.saturating_duration_since(t),
                    cpus,
                )
            }
            None => {
                *last = Some((now, cpu));
                None
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::time::Duration;

    pub fn memory_of(pid: u32) -> Option<u64> {
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a rusage_info_v2, the flavour asked for.
        let rc = unsafe {
            libc::proc_pid_rusage(
                pid as libc::c_int,
                libc::RUSAGE_INFO_V2,
                &mut info as *mut _ as *mut libc::rusage_info_t,
            )
        };
        if rc == 0 && info.ri_phys_footprint > 0 {
            return Some(info.ri_phys_footprint);
        }
        let mut task: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        // SAFETY: the buffer is a proc_taskinfo of the size passed.
        let n = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTASKINFO,
                0,
                &mut task as *mut _ as *mut libc::c_void,
                size,
            )
        };
        (n == size).then_some(task.pti_resident_size)
    }

    pub fn self_cpu_time() -> Option<Duration> {
        super::rusage_self()
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::time::Duration;

    pub fn memory_of(pid: u32) -> Option<u64> {
        let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let pages = super::resident_pages(&statm)?;
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        (page > 0).then(|| pages * page as u64)
    }

    pub fn self_cpu_time() -> Option<Duration> {
        super::rusage_self()
    }
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn working_set(h: HANDLE) -> Option<u64> {
        let mut pmc: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        pmc.cb = cb;
        // SAFETY: `pmc` is a PROCESS_MEMORY_COUNTERS of size `cb`.
        let ok = unsafe { GetProcessMemoryInfo(h, &mut pmc, cb) };
        (ok != 0).then_some(pmc.WorkingSetSize as u64)
    }

    pub fn memory_of(pid: u32) -> Option<u64> {
        if pid == std::process::id() {
            // SAFETY: the pseudo-handle needs no closing.
            return working_set(unsafe { GetCurrentProcess() });
        }
        // SAFETY: plain Win32 calls; the handle is closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return None;
        }
        let m = working_set(h);
        unsafe { CloseHandle(h) };
        m
    }

    fn ticks(ft: &FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }

    pub fn self_cpu_time() -> Option<Duration> {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: four valid FILETIME out-pointers.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        };
        // FILETIME counts 100 ns ticks.
        (ok != 0).then(|| Duration::from_nanos((ticks(&kernel) + ticks(&user)) * 100))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    use std::time::Duration;

    pub fn memory_of(_pid: u32) -> Option<u64> {
        None
    }

    pub fn self_cpu_time() -> Option<Duration> {
        None
    }
}

/// User + system CPU time of this process (all threads).
#[cfg(unix)]
fn rusage_self() -> Option<Duration> {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `ru` is a valid rusage out-pointer.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } != 0 {
        return None;
    }
    let tv = |t: libc::timeval| {
        Duration::from_secs(t.tv_sec.max(0) as u64) + Duration::from_micros(t.tv_usec.max(0) as u64)
    };
    Some(tv(ru.ru_utime) + tv(ru.ru_stime))
}

/// Resident pages from `/proc/<pid>/statm` ("size resident shared …").
/// Pure.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn resident_pages(statm: &str) -> Option<u64> {
    statm.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_percent_is_share_of_the_whole_machine() {
        let s = Duration::from_secs;
        // One core busy for the whole second on a 4-core machine: 25 %.
        assert_eq!(cpu_percent(s(1), s(1), 4), Some(25.0));
        assert_eq!(cpu_percent(s(4), s(1), 4), Some(100.0));
        // Clock jitter can't push it past the machine.
        assert_eq!(cpu_percent(s(5), s(1), 4), Some(100.0));
        assert_eq!(cpu_percent(s(1), Duration::ZERO, 4), None);
    }

    #[test]
    fn cpu_meter_needs_two_readings() {
        let m = CpuMeter::new();
        let t0 = Instant::now();
        assert_eq!(m.sample(t0, Duration::from_millis(100), 2), None);
        // Too soon: ignored, the first reading stays the baseline.
        assert_eq!(
            m.sample(
                t0 + Duration::from_millis(50),
                Duration::from_millis(150),
                2
            ),
            None
        );
        let pct = m
            .sample(t0 + Duration::from_secs(1), Duration::from_millis(600), 2)
            .unwrap();
        assert!((pct - 25.0).abs() < 1e-9, "{pct}");
    }

    #[test]
    fn statm_resident_field() {
        assert_eq!(resident_pages("123456 7890 321 1 0 4567 0\n"), Some(7890));
        assert_eq!(resident_pages("garbage"), None);
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    fn this_process_has_memory_and_cpu_time() {
        assert!(self_memory().unwrap() > 1_000_000);
        assert!(self_cpu_time().is_some());
    }
}
