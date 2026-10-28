//! What this host has left of its memory, read from its own kernel.
//!
//! Two readers, one per platform, and neither of them guesses. macOS answers
//! through `sysctl` (`hw.memsize`, `kern.memorystatus_level`, `vm.swapusage`)
//! and `vm_stat`; Linux answers through `/proc/meminfo`. A figure the host did
//! not answer is absent, and why is kept in `unread`, never a number nothing
//! measured.
//!
//! The two platforms do not mean the same thing by "free", and pretending
//! they do is how a reading becomes noise. Linux publishes `MemAvailable`,
//! which the kernel computes as what a new allocation can obtain without
//! swapping. macOS publishes its own counterpart as a percentage,
//! `kern.memorystatus_level` — the figure `memory_pressure` prints as "memory
//! free percentage" — and this reader takes that share of `hw.memsize`.
//! Summing free, speculative and purgeable pages leaves out the inactive and
//! file-backed cache the kernel reclaims, so a host could publish well under a
//! GiB available while the kernel's own level read most of its memory
//! reclaimable. The compressor and swapout counters are recorded beside it as
//! evidence.
//!
//! The text parsers are shared with `stado space report`, which folds the
//! same rows read from another host ([`from_meminfo`], [`from_macos`]), so a
//! report about a host and that host's own verdict are computed one way.

use std::process::Command;

use super::constants;

/// One host's memory state at one instant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryReading {
    /// Memory a new allocation can obtain without swapping, in bytes.
    pub available_bytes: Option<i64>,
    /// Physical memory installed, in bytes.
    pub total_bytes: Option<i64>,
    /// Swap in use, in bytes.
    pub swap_used_bytes: Option<i64>,
    /// Swap configured, in bytes.
    pub swap_total_bytes: Option<i64>,
    /// macOS only: pages held by the compressor.
    pub compressor_pages: Option<i64>,
    /// macOS only: lifetime swapouts.
    pub swapouts: Option<i64>,
    /// macOS only: the page size those page counts are in, so a reader can
    /// turn them into bytes without guessing it.
    pub page_size_bytes: Option<i64>,
    /// Why each figure that is absent was not read, joined; `None` when every
    /// figure the platform states was read.
    pub unread: Option<String>,
}

impl MemoryReading {
    /// Swap utilisation as a whole percentage, when both halves were read and
    /// some swap is configured.
    pub fn swap_used_pct(&self) -> Option<i64> {
        match (self.swap_used_bytes, self.swap_total_bytes) {
            (Some(used), Some(total)) if total.is_positive() => {
                Some(used.saturating_mul(constants::PERCENT) / total)
            }
            _ => None,
        }
    }
}

/// The causes of the figures one reading could not take.
#[derive(Default)]
struct Causes(Vec<String>);

impl Causes {
    fn take<T>(&mut self, read: Result<T, String>) -> Option<T> {
        match read {
            Ok(value) => Some(value),
            Err(cause) => {
                self.0.push(cause);
                None
            }
        }
    }

    fn into_unread(self) -> Option<String> {
        (!self.0.is_empty()).then(|| self.0.join("; "))
    }
}

/// Read this host's memory through its own kernel.
pub fn read_host_memory() -> MemoryReading {
    if cfg!(target_os = "macos") {
        from_macos(
            sysctl("hw.memsize"),
            sysctl(MACOS_MEMORY_LEVEL),
            command_stdout("/usr/bin/vm_stat", &[]),
            command_stdout("/usr/sbin/sysctl", &["-n", "vm.swapusage"]),
        )
    } else {
        match std::fs::read_to_string(MEMINFO) {
            Ok(text) => from_meminfo(&text),
            Err(error) => MemoryReading {
                unread: Some(format!("{MEMINFO} cannot be read: {error}")),
                ..MemoryReading::default()
            },
        }
    }
}

/// Available and installed memory alone, in bytes, without the swap and
/// compressor reads a full [`read_host_memory`] makes; why, when either was
/// not read.
pub fn read_available_and_total() -> Result<(i64, i64), String> {
    if cfg!(target_os = "macos") {
        let total = sysctl("hw.memsize")?;
        return Ok((macos_available(sysctl(MACOS_MEMORY_LEVEL)?, total)?, total));
    }
    let text = std::fs::read_to_string(MEMINFO)
        .map_err(|error| format!("{MEMINFO} cannot be read: {error}"))?;
    Ok((
        meminfo_bytes(&text, "MemAvailable")?,
        meminfo_bytes(&text, "MemTotal")?,
    ))
}

/// Where Linux states its memory.
const MEMINFO: &str = "/proc/meminfo";

fn command_stdout(program: &str, args: &[&str]) -> Result<String, String> {
    let shown = [program]
        .iter()
        .chain(args)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let output = crate::wait::output(Command::new(program).args(args))
        .map_err(|error| format!("{shown} did not run: {error}"))?;
    if !output.status.success() {
        return Err(format!("{shown} exited {}", output.status));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("{shown} printed text that is not UTF-8: {error}"))
}

/// One integer `sysctl -n` states.
fn sysctl(name: &str) -> Result<i64, String> {
    let text = command_stdout("/usr/sbin/sysctl", &["-n", name])?;
    text.trim().parse::<i64>().map_err(|error| {
        format!(
            "sysctl {name} printed {:?}, not an integer: {error}",
            text.trim()
        )
    })
}

/// The `sysctl` that states, as a whole percentage of physical memory, what
/// the macOS kernel counts as available: its counterpart of Linux
/// `MemAvailable`.
pub const MACOS_MEMORY_LEVEL: &str = "kern.memorystatus_level";
/// The macOS compressor's occupancy row.
pub const MACOS_COMPRESSOR_ROW: &str = "Pages occupied by compressor";
/// The macOS lifetime swapout row.
pub const MACOS_SWAPOUT_ROW: &str = "Swapouts";

/// The bytes a kernel memory level (a whole percentage) makes of `total`, or
/// why a level outside a percentage makes none.
pub fn macos_available(level_pct: i64, total: i64) -> Result<i64, String> {
    if level_pct.is_negative() || level_pct > constants::PERCENT {
        return Err(format!(
            "{MACOS_MEMORY_LEVEL} is {level_pct}, not a percentage"
        ));
    }
    Ok(total.saturating_mul(level_pct) / constants::PERCENT)
}

/// The figures macOS states, from its four answers as they came: each one
/// that failed leaves its figures absent with its cause in `unread`.
pub fn from_macos(
    total: Result<i64, String>,
    level: Result<i64, String>,
    vm_stat: Result<String, String>,
    swapusage: Result<String, String>,
) -> MemoryReading {
    let mut causes = Causes::default();
    let mut reading = MemoryReading {
        total_bytes: causes.take(total),
        ..MemoryReading::default()
    };
    if let (Some(level), Some(total)) = (causes.take(level), reading.total_bytes) {
        reading.available_bytes = causes.take(macos_available(level, total));
    }
    if let Some(text) = causes.take(vm_stat) {
        reading.compressor_pages = causes.take(vm_stat_pages(&text, MACOS_COMPRESSOR_ROW));
        reading.swapouts = causes.take(vm_stat_pages(&text, MACOS_SWAPOUT_ROW));
        reading.page_size_bytes = causes.take(vm_stat_page_size(&text));
    }
    if let Some((used, total)) = causes.take(swapusage.and_then(|text| swapusage_bytes(&text))) {
        reading.swap_used_bytes = Some(used);
        reading.swap_total_bytes = Some(total);
    }
    reading.unread = causes.into_unread();
    reading
}

/// The `vm_stat` banner states its page size as `page size of N bytes`.
pub fn vm_stat_page_size(text: &str) -> Result<i64, String> {
    const MARKER: &str = "page size of ";
    let banner = text.lines().next().ok_or("vm_stat printed nothing")?;
    let number = banner
        .split_once(MARKER)
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .ok_or_else(|| format!("vm_stat's banner {banner:?} states no page size"))?;
    number
        .parse::<i64>()
        .map_err(|error| format!("vm_stat's page size {number:?} is not an integer: {error}"))
}

/// A `vm_stat` row states its label, a colon, its count and a period.
pub fn vm_stat_pages(text: &str, label: &str) -> Result<i64, String> {
    let value = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim() == label)
        .map(|(_, value)| value.trim().trim_end_matches('.'))
        .ok_or_else(|| format!("vm_stat printed no {label:?} row"))?;
    value
        .parse::<i64>()
        .map_err(|error| format!("vm_stat's {label:?} row {value:?} is not a count: {error}"))
}

/// `vm.swapusage` states `total`, `used` and `free` as suffixed sizes; the
/// used and total bytes.
pub fn swapusage_bytes(text: &str) -> Result<(i64, i64), String> {
    let field = |name: &str| -> Result<i64, String> {
        let token = text
            .split_once(&format!("{name} = "))
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .ok_or_else(|| format!("vm.swapusage {:?} states no {name} size", text.trim()))?;
        parse_suffixed_size(token)
    };
    Ok((field("used")?, field("total")?))
}

/// A `K`, `M` or `G` suffixed decimal size into bytes; a size without a
/// suffix is a count of bytes.
pub fn parse_suffixed_size(token: &str) -> Result<i64, String> {
    let decimal = |digits: &str| -> Result<f64, String> {
        digits
            .parse::<f64>()
            .map_err(|error| format!("size {token:?} is not a decimal number: {error}"))
    };
    let scaled =
        |digits: &str, scale: i64| decimal(digits).map(|value| (value * scale as f64) as i64);
    match token.char_indices().last() {
        Some((at, 'K')) => scaled(&token[..at], constants::KIB),
        Some((at, 'M')) => scaled(&token[..at], constants::MIB),
        Some((at, 'G')) => scaled(&token[..at], constants::GIB),
        _ => decimal(token).map(|bytes| bytes as i64),
    }
}

/// The figures `/proc/meminfo` states: available, installed and swap.
pub fn from_meminfo(text: &str) -> MemoryReading {
    let mut causes = Causes::default();
    let mut reading = MemoryReading {
        available_bytes: causes.take(meminfo_bytes(text, "MemAvailable")),
        total_bytes: causes.take(meminfo_bytes(text, "MemTotal")),
        ..MemoryReading::default()
    };
    let swap_total = causes.take(meminfo_bytes(text, "SwapTotal"));
    let swap_free = causes.take(meminfo_bytes(text, "SwapFree"));
    reading.swap_total_bytes = swap_total;
    if let (Some(total), Some(free)) = (swap_total, swap_free) {
        reading.swap_used_bytes = Some(total.saturating_sub(free));
    }
    reading.unread = causes.into_unread();
    reading
}

/// One `/proc/meminfo` row, which states kibibytes, in bytes.
pub fn meminfo_bytes(text: &str, label: &str) -> Result<i64, String> {
    let number = text
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim() == label)
        .and_then(|(_, value)| value.split_whitespace().next())
        .ok_or_else(|| format!("{MEMINFO} states no {label} figure"))?;
    number
        .parse::<i64>()
        .map(|kib| kib.saturating_mul(constants::KIB))
        .map_err(|error| format!("{MEMINFO}'s {label} figure {number:?} is not a count: {error}"))
}
