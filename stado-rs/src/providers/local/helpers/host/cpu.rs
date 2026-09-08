//! Processor capacity, measured from processor-time deltas.
//!
//! The cached sample below is the whole reason this is not a one-liner: a
//! single reading of the kernel's cumulative counters says nothing, so the
//! first observation stores a baseline and only the second one answers.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// CPU cores the current process may schedule on. This respects cgroup and
/// affinity limits through [`std::thread::available_parallelism`].
pub fn total_cpu_cores() -> i64 {
    std::thread::available_parallelism()
        .map(|count| count.get() as i64)
        .unwrap_or(1)
}

/// One-minute host load, or `None` when the operating system cannot provide it.
pub fn load_average_1m() -> Option<f64> {
    let mut values = [0.0_f64; 1];
    // SAFETY: `values` has the one writable element requested from getloadavg.
    let read = unsafe { nix::libc::getloadavg(values.as_mut_ptr(), values.len() as i32) };
    (read == 1 && values[0].is_finite() && values[0] >= 0.0).then_some(values[0])
}

#[derive(Clone, Copy)]
struct CpuTime {
    total: u64,
    idle: u64,
}

struct CpuSample {
    time: CpuTime,
    observed_at: Instant,
    busy_fraction: Option<f64>,
}

static CPU_SAMPLE: Mutex<Option<CpuSample>> = Mutex::new(None);

fn cpu_busy_fraction() -> Option<f64> {
    let mut previous = CPU_SAMPLE.lock().ok()?;
    let now = Instant::now();
    if let Some(sample) = previous.as_ref() {
        if now.duration_since(sample.observed_at) < Duration::from_millis(250) {
            return sample.busy_fraction;
        }
    }
    let Some(time) = cpu_time() else {
        *previous = None;
        return None;
    };
    let busy_fraction = previous.as_ref().and_then(|sample| {
        let total = time.total.checked_sub(sample.time.total)?;
        let idle = time.idle.checked_sub(sample.time.idle)?;
        (total > 0 && idle <= total).then(|| 1.0 - idle as f64 / total as f64)
    });
    *previous = Some(CpuSample {
        time,
        observed_at: now,
        busy_fraction,
    });
    busy_fraction
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn cpu_time() -> Option<CpuTime> {
    let mut stats: nix::libc::host_cpu_load_info_data_t = unsafe { std::mem::zeroed() };
    let mut count = nix::libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: both pointers reference initialized, correctly sized writable
    // storage and Mach writes at most HOST_CPU_LOAD_INFO_COUNT integer words.
    let status = unsafe {
        nix::libc::host_statistics(
            nix::libc::mach_host_self(),
            nix::libc::HOST_CPU_LOAD_INFO,
            (&mut stats as *mut nix::libc::host_cpu_load_info_data_t)
                .cast::<nix::libc::integer_t>(),
            &mut count,
        )
    };
    if status != nix::libc::KERN_SUCCESS || count != nix::libc::HOST_CPU_LOAD_INFO_COUNT {
        return None;
    }
    Some(CpuTime {
        total: stats.cpu_ticks.iter().map(|ticks| u64::from(*ticks)).sum(),
        idle: u64::from(stats.cpu_ticks[nix::libc::CPU_STATE_IDLE as usize]),
    })
}

#[cfg(not(target_os = "macos"))]
fn cpu_time() -> Option<CpuTime> {
    use std::io::BufRead;

    let file = std::fs::File::open("/proc/stat").ok()?;
    let mut line = String::new();
    std::io::BufReader::new(file).read_line(&mut line).ok()?;
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let mut ticks = [0_u64; 8];
    for value in &mut ticks {
        *value = fields.next()?.parse().ok()?;
    }
    // guest and guest_nice already belong to user and nice. I/O wait does
    // not consume processor time; steal does consume this VM's capacity.
    Some(CpuTime {
        total: ticks.iter().sum(),
        idle: ticks[3] + ticks[4],
    })
}

/// CPU capacity derived from processor-time deltas and jobs already owned by
/// this agent, never runnable-process load averages. The first observation
/// waits for a second sample rather than claiming unmeasured CPU is idle.
pub fn available_cpu_cores(active_requested_cores: i64) -> Option<i64> {
    let total = total_cpu_cores();
    let observed_busy = (cpu_busy_fraction()? * total as f64).ceil() as i64;
    Some(
        total
            .saturating_sub(active_requested_cores.max(observed_busy))
            .max(0),
    )
}
