//! The memory half of `stado space report TARGET`: what the host has left.
//!
//! Free pages and the swap line alone, on macOS only, cannot explain a
//! runner install failing with `Failed to create CoreCLR, HRESULT:
//! 0x8007000C` while the report says the disk has room: the cause is memory
//! and swap, and a machine in that state has already pushed everything it
//! can into the compressor and onto disk. So the reader answers total
//! memory, obtainable memory, swap used and swap total, the compressor and
//! the lifetime swapouts, on both platforms, and why each one it could not
//! read is absent.

use super::*;

/// Memory and swap, read from the host's own kernel.
///
/// Two fixed, read-only readers, one per platform, and a host missing either
/// reports what it has. On macOS available memory is the kernel's own
/// `kern.memorystatus_level` share of `hw.memsize`, the same figure the
/// host's capacity publication reads
/// ([`crate::providers::local::host_memory::reading`]). The `vm_stat` banner
/// row travels with the page rows, so the page size is read from the banner
/// the way the host's own pass reads it.
pub const MEMORY_SECTION: &str = r#"if [ -x /usr/bin/vm_stat ]; then
  /usr/bin/vm_stat 2>/dev/null | while IFS= read -r row; do
    case "$row" in
      Mach*page\ size\ of*|Pages\ occupied\ by\ compressor:*|Swapouts:*)
        printf 'STADO_MEMORY_PAGES\t%s\n' "$row" ;;
    esac
  done
fi
if [ -x /usr/sbin/sysctl ]; then
  printf 'STADO_MEMORY\ttotal_bytes\t%s\n' "$(/usr/sbin/sysctl -n hw.memsize 2>/dev/null)"
  printf 'STADO_MEMORY\tlevel_pct\t%s\n' "$(/usr/sbin/sysctl -n kern.memorystatus_level 2>/dev/null)"
  /usr/sbin/sysctl vm.swapusage 2>/dev/null | while IFS= read -r row; do
    case "$row" in
      vm.swapusage:*) printf 'STADO_MEMORY\tswap\t%s\n' "${row#vm.swapusage: }" ;;
    esac
  done
fi
if [ -r /proc/meminfo ]; then
  while IFS= read -r row; do
    case "$row" in
      MemTotal:*|MemAvailable:*|SwapTotal:*|SwapFree:*) printf 'STADO_MEMINFO\t%s\n' "$row" ;;
    esac
  done < /proc/meminfo
fi
"#;

/// The memory rows one host printed, as it printed them, for
/// [`fold_memory`].
#[derive(Debug, Default)]
pub(super) struct MemoryMarkers {
    /// `sysctl -n hw.memsize`, when the host has `sysctl`.
    pub(super) total_bytes: Option<String>,
    /// `sysctl -n kern.memorystatus_level`.
    pub(super) level_pct: Option<String>,
    /// The `vm.swapusage` value.
    pub(super) swap: Option<String>,
    /// The `vm_stat` banner and the compressor and swapout rows, in order.
    pub(super) pages: Vec<String>,
    /// The `/proc/meminfo` rows, when the host has one.
    pub(super) meminfo: Vec<String>,
}

impl MemoryMarkers {
    /// Keep one marker line's fields when they are a memory row; whether
    /// they were.
    pub(super) fn take(&mut self, fields: &[&str]) -> bool {
        match fields {
            ["STADO_MEMORY", "level_pct", value] => self.level_pct = Some((*value).to_string()),
            ["STADO_MEMORY", "total_bytes", value] => self.total_bytes = Some((*value).to_string()),
            ["STADO_MEMORY", "swap", value] => self.swap = Some((*value).to_string()),
            ["STADO_MEMORY_PAGES", row] => self.pages.push((*row).to_string()),
            ["STADO_MEMINFO", row] => self.meminfo.push((*row).to_string()),
            _ => return false,
        }
        true
    }
}

/// The memory a script that ran [`MEMORY_SECTION`] printed, folded the way
/// `space report` folds it: every other command that reports a host's memory
/// prints the same section and reads it here, so no two reports disagree
/// about what "available" is.
pub fn memory_from_output(stdout: &str) -> MemoryReading {
    let mut markers = MemoryMarkers::default();
    for line in stdout.lines() {
        markers.take(&host_channel::marker_fields(line));
    }
    fold_memory(markers)
}

/// The `memory` block of the report: what the host's kernel says it has.
/// Readings only; no watermark is applied and nothing reclaims memory.
pub fn memory_report(reading: &DiskReading) -> Value {
    memory_json(&reading.memory)
}

/// One memory reading as every report prints it.
pub fn memory_json(memory: &MemoryReading) -> Value {
    json!({
        "free_kb": available_kb(memory),
        "swap": swap_line(memory),
        "available_bytes": memory.available_bytes,
        "available_mb": memory
            .available_bytes
            .map(|bytes| bytes / crate::providers::local::host_memory::constants::MIB),
        "total_bytes": memory.total_bytes,
        "swap_used_bytes": memory.swap_used_bytes,
        "swap_total_bytes": memory.swap_total_bytes,
        "swap_used_pct": memory.swap_used_pct(),
        "compressor_pages": memory.compressor_pages,
        "swapouts": memory.swapouts,
        "page_size_bytes": memory.page_size_bytes,
        "unread": memory.unread,
    })
}

/// The obtainable memory in kibibytes, as the first version of this report
/// spelled it: a decimal string, or absent when the host did not answer.
pub(super) fn available_kb(memory: &MemoryReading) -> Option<String> {
    let kib = crate::providers::local::host_memory::constants::KIB;
    memory
        .available_bytes
        .map(|bytes| (bytes / kib).to_string())
}

/// The swap line as `vm.swapusage` spelled it, rebuilt from the two figures
/// the reader now keeps separately.
pub(super) fn swap_line(memory: &MemoryReading) -> Option<String> {
    let mib = crate::providers::local::host_memory::constants::MIB;
    match (memory.swap_used_bytes, memory.swap_total_bytes) {
        (Some(used), Some(total)) => Some(format!(
            "total = {}M  used = {}M  free = {}M",
            total / mib,
            used / mib,
            total.saturating_sub(used) / mib
        )),
        _ => None,
    }
}

/// Turn the platform rows into the same figures the host's own pass reads.
///
/// The folds are [`crate::providers::local::host_memory::reading`]'s own, so
/// that a report about a host and that host's own watermark verdict cannot be
/// computed two different ways, and a figure the host did not print is
/// absent with the reason beside it in `unread`.
pub(super) fn fold_memory(markers: MemoryMarkers) -> MemoryReading {
    use crate::providers::local::host_memory::reading as host;
    if !markers.meminfo.is_empty() {
        return host::from_meminfo(&markers.meminfo.join("\n"));
    }
    let printed =
        |row: Option<String>, what: &str| row.ok_or_else(|| format!("the host printed no {what}"));
    let integer = |row: Option<String>, what: &str| {
        printed(row, what).and_then(|value| {
            value.trim().parse::<i64>().map_err(|error| {
                format!(
                    "the host's {what} {:?} is not an integer: {error}",
                    value.trim()
                )
            })
        })
    };
    let vm_stat = if markers.pages.is_empty() {
        Err("the host printed no vm_stat rows".to_string())
    } else {
        Ok(markers.pages.join("\n"))
    };
    host::from_macos(
        integer(markers.total_bytes, "hw.memsize"),
        integer(markers.level_pct, host::MACOS_MEMORY_LEVEL),
        vm_stat,
        printed(markers.swap, "vm.swapusage"),
    )
}
