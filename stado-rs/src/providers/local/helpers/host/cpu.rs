//! Processor capacity, measured from processor-time deltas.
//!
//! The cached sample below is the whole reason this is not a one-liner: a
//! single reading of the kernel's cumulative counters says nothing, so the
//! first observation stores a baseline and only the second one answers.
//! Every reading the operating system refuses is answered with its refusal,
//! so a host publishes why its CPU is unmeasured instead of a guessed count.

use std::sync::Mutex;

/// CPU cores the current process may schedule on, as
/// [`std::thread::available_parallelism`] reads them through cgroup and
/// affinity limits, or the operating system's refusal.
pub fn total_cpu_cores() -> Result<i64, String> {
    let count = std::thread::available_parallelism().map_err(|error| {
        format!("the cores this process may schedule on cannot be read: {error}")
    })?;
    i64::try_from(count.get()).map_err(|error| {
        format!("{count} schedulable cores do not fit in a signed 64-bit count: {error}")
    })
}

/// One-minute host load, or `None` when the operating system cannot provide it.
pub fn load_average_1m() -> Option<f64> {
    let mut average = [f64::NAN];
    // SAFETY: `average` has the one writable element requested from getloadavg.
    let read = unsafe { nix::libc::getloadavg(average.as_mut_ptr(), average.len() as i32) };
    let [value] = average;
    (usize::try_from(read).is_ok_and(|count| count == average.len())
        && value.is_finite()
        && !value.is_sign_negative())
    .then_some(value)
}

#[derive(Clone, Copy)]
struct CpuTime {
    total: u64,
    idle: u64,
}

struct CpuSample {
    time: CpuTime,
    busy_fraction: Option<f64>,
}

static CPU_SAMPLE: Mutex<Option<CpuSample>> = Mutex::new(None);

/// The share of processor time spent busy since the kept baseline, `None`
/// while the first sample is only a baseline. The kernel's counters move in
/// whole clock ticks, so until one has passed since the baseline the last
/// answer stands and the baseline is kept: two readings inside one tick say
/// nothing, and no wait of a chosen length decides that. A counter that
/// cannot be read drops the baseline and answers why.
fn cpu_busy_fraction() -> Result<Option<f64>, String> {
    // The sample is replaced whole, so a panic while it was held leaves a
    // usable baseline; left poisoned, the host would report no CPU reading
    // for as long as the agent runs.
    let mut previous = match CPU_SAMPLE.lock() {
        Ok(previous) => previous,
        Err(poisoned) => poisoned.into_inner(),
    };
    let time = match cpu_time() {
        Ok(time) => time,
        Err(cause) => {
            *previous = None;
            return Err(cause);
        }
    };
    if let Some(sample) = previous.as_ref() {
        if time.total == sample.time.total {
            return Ok(sample.busy_fraction);
        }
    }
    let busy_fraction = previous.as_ref().and_then(|sample| {
        let total = std::num::NonZeroU64::new(time.total.checked_sub(sample.time.total)?)?;
        let busy = total
            .get()
            .checked_sub(time.idle.checked_sub(sample.time.idle)?)?;
        Some(busy as f64 / total.get() as f64)
    });
    *previous = Some(CpuSample {
        time,
        busy_fraction,
    });
    Ok(busy_fraction)
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn cpu_time() -> Result<CpuTime, String> {
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
    if status != nix::libc::KERN_SUCCESS {
        return Err(format!(
            "host_statistics(HOST_CPU_LOAD_INFO) answered kern_return_t {status}"
        ));
    }
    if count != nix::libc::HOST_CPU_LOAD_INFO_COUNT {
        return Err(format!(
            "host_statistics(HOST_CPU_LOAD_INFO) wrote {count} words where {} were asked for",
            nix::libc::HOST_CPU_LOAD_INFO_COUNT
        ));
    }
    Ok(CpuTime {
        total: stats.cpu_ticks.iter().map(|ticks| u64::from(*ticks)).sum(),
        idle: u64::from(stats.cpu_ticks[nix::libc::CPU_STATE_IDLE as usize]),
    })
}

/// The next field of /proc/stat's aggregate `cpu` line as a tick count, or
/// why it is not one.
#[cfg(not(target_os = "macos"))]
fn tick(fields: &mut std::str::SplitWhitespace<'_>, name: &str) -> Result<u64, String> {
    let field = fields
        .next()
        .ok_or_else(|| format!("/proc/stat's cpu line ends before its {name} field"))?;
    field.parse().map_err(|error| {
        format!("/proc/stat's cpu {name} field {field:?} is not a tick count: {error}")
    })
}

/// The aggregate `cpu` line of /proc/stat, read by the fields proc(5) lists
/// in order: <https://man7.org/linux/man-pages/man5/proc_stat.5.html>.
#[cfg(not(target_os = "macos"))]
fn cpu_time() -> Result<CpuTime, String> {
    use std::io::BufRead;

    let file = std::fs::File::open("/proc/stat")
        .map_err(|error| format!("/proc/stat cannot be opened: {error}"))?;
    let mut line = String::new();
    std::io::BufReader::new(file)
        .read_line(&mut line)
        .map_err(|error| format!("/proc/stat cannot be read: {error}"))?;
    let mut fields = line.split_whitespace();
    if fields.next() != Some("cpu") {
        return Err(format!(
            "/proc/stat does not begin with its aggregate cpu line: {line:?}"
        ));
    }
    let user = tick(&mut fields, "user")?;
    let nice = tick(&mut fields, "nice")?;
    let system = tick(&mut fields, "system")?;
    let idle = tick(&mut fields, "idle")?;
    let iowait = tick(&mut fields, "iowait")?;
    let irq = tick(&mut fields, "irq")?;
    let softirq = tick(&mut fields, "softirq")?;
    let steal = tick(&mut fields, "steal")?;
    // guest and guest_nice already belong to user and nice. I/O wait does
    // not consume processor time; steal does consume this VM's capacity.
    Ok(CpuTime {
        total: user + nice + system + idle + iowait + irq + softirq + steal,
        idle: idle + iowait,
    })
}

/// CPU capacity derived from processor-time deltas and jobs already owned by
/// this agent, never runnable-process load averages: `None` while the first
/// sample is only a baseline, rather than claiming unmeasured CPU is idle,
/// and the operating system's refusal when a reading fails. Jobs that ask for
/// more than the host has leave it negative.
pub fn available_cpu_cores(active_requested_cores: i64) -> Result<Option<i64>, String> {
    let total = total_cpu_cores()?;
    let Some(busy) = cpu_busy_fraction()? else {
        return Ok(None);
    };
    let observed_busy = (busy * total as f64).ceil() as i64;
    Ok(Some(
        total.saturating_sub(active_requested_cores.max(observed_busy)),
    ))
}
