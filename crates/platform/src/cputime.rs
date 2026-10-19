//! Processor time used by this process and by the calling thread (user + kernel), for the
//! performance counters (`perf.stats` `process.cpuS`) and the playback profiler.
//!
//! Windows asks the system (`GetProcessTimes` / `GetThreadTimes`); Unix reads the CPU-time clocks
//! (`clock_gettime`, through `rustix` in `filmcraft-ui-egui`), so this module answers `None` there.
//! Diagnostics only: nothing a project produces depends on it (ADR 0001, amendment 1).

use std::time::Duration;

/// CPU time consumed by the whole process, every thread (None where this module cannot read it).
pub fn process_cpu_time() -> Option<Duration> {
    #[cfg(windows)]
    {
        win::process()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// CPU time consumed by the calling thread (None where this module cannot read it).
pub fn thread_cpu_time() -> Option<Duration> {
    #[cfg(windows)]
    {
        win::thread()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// A `FILETIME` count of 100 ns units as a duration.
#[cfg_attr(not(windows), allow(dead_code))]
fn from_100ns(units: u64) -> Duration {
    Duration::new(units / 10_000_000, ((units % 10_000_000) * 100) as u32)
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use std::time::Duration;

    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, GetProcessTimes, GetThreadTimes};

    fn units(t: FILETIME) -> u64 {
        (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
    }

    fn total(kernel: FILETIME, user: FILETIME) -> Duration {
        super::from_100ns(units(kernel).saturating_add(units(user)))
    }

    pub fn process() -> Option<Duration> {
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: `GetCurrentProcess` returns a pseudo-handle that is always valid for the calling
        // process and needs no closing; the four out-pointers are distinct, live, writable locals.
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
        Some(total(kernel, user))
    }

    pub fn thread() -> Option<Duration> {
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        // SAFETY: `GetCurrentThread` returns a pseudo-handle that is always valid for the calling
        // thread and needs no closing; the four out-pointers are distinct, live, writable locals.
        unsafe { GetThreadTimes(GetCurrentThread(), &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
        Some(total(kernel, user))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hundred_nanosecond_units() {
        assert_eq!(from_100ns(0), Duration::ZERO);
        assert_eq!(from_100ns(10_000_000), Duration::from_secs(1));
        assert_eq!(from_100ns(15_000_001), Duration::new(1, 500_000_100));
        assert_eq!(from_100ns(u64::MAX).as_secs(), u64::MAX / 10_000_000);
    }

    #[test]
    fn cpu_time_grows_with_work() {
        let (Some(p0), Some(t0)) = (process_cpu_time(), thread_cpu_time()) else {
            #[cfg(windows)]
            panic!("Windows reports CPU time");
            #[cfg(not(windows))]
            return;
        };
        // spin until the thread has used measurable CPU (the clocks tick in ~16 ms steps)
        let start = std::time::Instant::now();
        let mut x = 0u64;
        while thread_cpu_time().is_some_and(|t| t <= t0) && start.elapsed() < Duration::from_secs(5) {
            x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
        }
        assert!(thread_cpu_time().is_some_and(|t| t > t0), "thread CPU time advanced");
        assert!(process_cpu_time().is_some_and(|p| p > p0), "process CPU time advanced");
        assert!(process_cpu_time() >= thread_cpu_time(), "the process includes this thread");
    }
}
