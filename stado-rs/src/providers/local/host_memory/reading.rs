//! What this host has left of its memory, read from its own kernel.
//!
//! Two readers, one per platform, and neither of them guesses. macOS answers
//! through `vm_stat` and `sysctl vm.swapusage`; Linux answers through
//! `/proc/meminfo`. A host that cannot answer a field reports that field as
//! absent rather than a number nothing measured.
//!
//! The two platforms do not mean the same thing by "free", and pretending
//! they do is how a reading becomes noise. Linux publishes `MemAvailable`,
//! which the kernel computes as what a new allocation can obtain without
//! swapping. macOS publishes its own counterpart as a percentage,
//! `kern.memorystatus_level` — the figure `memory_pressure` prints as "memory
//! free percentage" — and this reader takes that share of `hw.memsize`. It
//! used to sum only free, speculative and purgeable pages, which leaves out
//! the inactive and file-backed cache the kernel reclaims, so a host could
//! publish well under a GiB available while the kernel's own level read most
//! of its memory reclaimable. The compressor and swapout
//! counters are still recorded beside it as evidence.

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
}

impl MemoryReading {
    /// Swap utilisation as a whole percentage, when both halves were read.
    pub fn swap_used_pct(&self) -> Option<i64> {
        match (self.swap_used_bytes, self.swap_total_bytes) {
            (Some(used), Some(total)) if total > 0 => {
                Some(used.saturating_mul(constants::PERCENT) / total)
            }
            _ => None,
        }
    }
}

/// Read this host's memory through its own kernel.
pub fn read_host_memory() -> MemoryReading {
    if cfg!(target_os = "macos") {
        read_macos()
    } else {
        read_linux()
    }
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = crate::wait::output(Command::new(program).args(args)).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// The `sysctl` that states, as a whole percentage of physical memory, what
/// the macOS kernel counts as available: its counterpart of Linux
/// `MemAvailable`.
pub const MACOS_MEMORY_LEVEL: &str = "kern.memorystatus_level";
/// The macOS compressor's occupancy row.
pub const MACOS_COMPRESSOR_ROW: &str = "Pages occupied by compressor";
/// The macOS lifetime swapout row.
pub const MACOS_SWAPOUT_ROW: &str = "Swapouts";

/// The bytes a kernel memory level (a whole percentage) makes of `total`.
pub fn macos_available_bytes(level_pct: i64, total: i64) -> Option<i64> {
    (0..=constants::PERCENT)
        .contains(&level_pct)
        .then(|| total.saturating_mul(level_pct) / constants::PERCENT)
}

fn read_macos() -> MemoryReading {
    let mut reading = MemoryReading {
        total_bytes: command_stdout("/usr/sbin/sysctl", &["-n", "hw.memsize"])
            .and_then(|text| text.trim().parse::<i64>().ok()),
        ..MemoryReading::default()
    };
    let level = command_stdout("/usr/sbin/sysctl", &["-n", MACOS_MEMORY_LEVEL])
        .and_then(|text| text.trim().parse::<i64>().ok());
    reading.available_bytes = level
        .zip(reading.total_bytes)
        .and_then(|(level, total)| macos_available_bytes(level, total));
    if let Some(text) = command_stdout("/usr/bin/vm_stat", &[]) {
        reading.compressor_pages = vm_stat_pages(&text, MACOS_COMPRESSOR_ROW);
        reading.swapouts = vm_stat_pages(&text, MACOS_SWAPOUT_ROW);
        reading.page_size_bytes = vm_stat_page_size(&text);
    }
    if let Some(text) = command_stdout("/usr/sbin/sysctl", &["-n", "vm.swapusage"]) {
        let (used, total) = swapusage_bytes(&text);
        reading.swap_used_bytes = used;
        reading.swap_total_bytes = total;
    }
    reading
}

/// The `vm_stat` banner states its page size as `page size of N bytes`.
pub fn vm_stat_page_size(text: &str) -> Option<i64> {
    let marker = "page size of ";
    let rest = text.lines().next()?.split(marker).nth(1)?;
    rest.split_whitespace().next()?.parse::<i64>().ok()
}

/// A `vm_stat` row states its label, a colon, its count and a period.
pub fn vm_stat_pages(text: &str, label: &str) -> Option<i64> {
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim() != label {
            continue;
        }
        return value.trim().trim_end_matches('.').parse::<i64>().ok();
    }
    None
}

/// `vm.swapusage` states `total`, `used` and `free` as suffixed sizes.
pub fn swapusage_bytes(text: &str) -> (Option<i64>, Option<i64>) {
    let field = |name: &str| -> Option<i64> {
        let rest = text.split(&format!("{name} = ")).nth(1)?;
        let token = rest.split_whitespace().next()?;
        parse_suffixed_size(token)
    };
    (field("used"), field("total"))
}

/// A `K`, `M` or `G` suffixed decimal size into bytes.
pub fn parse_suffixed_size(token: &str) -> Option<i64> {
    let (digits, unit) = token.split_at(token.len().checked_sub(1)?);
    let kib = constants::MIB / 1024;
    let scale = match unit {
        "K" => kib,
        "M" => constants::MIB,
        "G" => constants::MIB.saturating_mul(kib),
        _ => return token.parse::<f64>().ok().map(|value| value as i64),
    };
    let value: f64 = digits.parse().ok()?;
    Some((value * scale as f64) as i64)
}

/// `/proc/meminfo` states every figure in kibibytes.
fn read_linux() -> MemoryReading {
    let mut reading = MemoryReading::default();
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return reading;
    };
    reading.available_bytes = meminfo_bytes(&text, "MemAvailable");
    reading.total_bytes = meminfo_bytes(&text, "MemTotal");
    let swap_total = meminfo_bytes(&text, "SwapTotal");
    let swap_free = meminfo_bytes(&text, "SwapFree");
    reading.swap_total_bytes = swap_total;
    reading.swap_used_bytes = match (swap_total, swap_free) {
        (Some(total), Some(free)) => Some(total.saturating_sub(free)),
        _ => None,
    };
    reading
}

/// One `/proc/meminfo` row, converted from kibibytes to bytes.
pub fn meminfo_bytes(text: &str, label: &str) -> Option<i64> {
    let kib = constants::MIB / 1024;
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim() != label {
            continue;
        }
        return value
            .split_whitespace()
            .next()
            .and_then(|number| number.parse::<i64>().ok())
            .map(|value| value.saturating_mul(kib));
    }
    None
}
