//! What this host has left of its memory, read from its own kernel.
//!
//! Two readers, one per platform, and neither of them guesses. macOS answers
//! through `vm_stat` and `sysctl vm.swapusage`; Linux answers through
//! `/proc/meminfo`. A host that cannot answer a field reports that field as
//! absent — the pass then has no watermark verdict to make and says so,
//! rather than producing a number nothing measured.
//!
//! The two platforms do not mean the same thing by "free", and pretending
//! they do is how a watermark becomes noise. Linux publishes `MemAvailable`,
//! which the kernel computes as what a new allocation can obtain without
//! swapping. macOS publishes no such figure, so this reader sums the page
//! classes that are obtainable without evicting anonymous memory — free,
//! speculative and purgeable — and records the compressor and swapout
//! counters beside it as evidence. On charless-mac-mini on 2026-09-06 those
//! counters read 797k pages in the compressor and 12.2M swapouts against 4.3
//! of 5 GB of swap in use, which is the state a free-page figure alone
//! reports as a merely busy machine.

use std::process::Command;

use super::constants;
use super::schema::MemoryReclaimPolicy;

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

    /// Available memory in whole MiB, when it was read.
    pub fn available_mb(&self) -> Option<i64> {
        self.available_bytes.map(|bytes| bytes / constants::MIB)
    }

    /// Whether this host is over either declared watermark.
    ///
    /// `None` when neither watermark could be evaluated, which is not the
    /// same answer as `false`: a pass that cannot read the host must not
    /// report it as healthy, and the outcome vocabulary carries
    /// `invalid_or_unavailable_policy` for a pass with no reading at all.
    pub fn over_watermark(&self, policy: &MemoryReclaimPolicy) -> Option<bool> {
        let by_memory = self
            .available_bytes
            .map(|available| available < policy.low_free_bytes());
        let by_swap = self
            .swap_used_pct()
            .map(|pct| pct >= policy.high_swap_used_pct);
        match (by_memory, by_swap) {
            (None, None) => None,
            (memory, swap) => Some(memory.unwrap_or(false) || swap.unwrap_or(false)),
        }
    }

    /// Whether this reading withholds the host from job selection, against
    /// the two watermarks it is measured with.
    ///
    /// Memory scarcity decides; swap decides only where availability could
    /// not be read at all. Used swap is history rather than pressure - Linux
    /// never pages anonymous memory back in on its own - so a host that
    /// swapped during one spike reads over its swap watermark for as long as
    /// it stays up. On 2026-09-10 `ubuntu-server-rtx-pro-6000` held 67.2 GB
    /// available of 132.1 GB against an 8 GiB watermark with 85% of an 8.59 GB
    /// swap file in use, refused every job, and `skarbiec` could not build
    /// `linux-amd64` in three consecutive releases. Withholding a host with
    /// 64 GiB of headroom frees no memory; it removes the fleet's one Linux
    /// builder. `over_watermark` still reports either crossing as pressure,
    /// and the declared repairs still run.
    ///
    /// One predicate, both writers: the pass records its answer in the report
    /// and the publisher answers the capacity document with it, so the report
    /// and the publication can never disagree about whether work is refused.
    pub fn withholds_placement(&self, low_bytes: i64, high_swap_used_pct: i64) -> Option<bool> {
        match (
            self.available_bytes.map(|available| available < low_bytes),
            self.swap_used_pct().map(|pct| pct >= high_swap_used_pct),
        ) {
            (Some(memory), _) => Some(memory),
            (None, Some(swap)) => Some(swap),
            (None, None) => None,
        }
    }

    /// Whether the host has reached the declared target watermark.
    pub fn at_target(&self, policy: &MemoryReclaimPolicy) -> Option<bool> {
        self.available_bytes
            .map(|available| available >= policy.target_free_bytes())
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
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// The page classes a new macOS allocation can obtain without evicting
/// anonymous memory.
pub const MACOS_OBTAINABLE_CLASSES: [&str; 3] =
    ["Pages free", "Pages speculative", "Pages purgeable"];
/// The macOS compressor's occupancy row.
pub const MACOS_COMPRESSOR_ROW: &str = "Pages occupied by compressor";
/// The macOS lifetime swapout row.
pub const MACOS_SWAPOUT_ROW: &str = "Swapouts";

fn read_macos() -> MemoryReading {
    let mut reading = MemoryReading {
        total_bytes: command_stdout("/usr/sbin/sysctl", &["-n", "hw.memsize"])
            .and_then(|text| text.trim().parse::<i64>().ok()),
        ..MemoryReading::default()
    };
    if let Some(text) = command_stdout("/usr/bin/vm_stat", &[]) {
        reading.compressor_pages = vm_stat_pages(&text, MACOS_COMPRESSOR_ROW);
        reading.swapouts = vm_stat_pages(&text, MACOS_SWAPOUT_ROW);
        if let Some(page_size) = vm_stat_page_size(&text) {
            let obtainable: i64 = MACOS_OBTAINABLE_CLASSES
                .iter()
                .filter_map(|label| vm_stat_pages(&text, label))
                .sum();
            reading.available_bytes = Some(obtainable.saturating_mul(page_size));
        }
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
