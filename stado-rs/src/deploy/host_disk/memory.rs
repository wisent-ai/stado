//! The memory half of `stado space report TARGET`: what the host has left.
//!
//! Free pages and the swap line alone, on macOS only, cannot explain a
//! runner install failing with `Failed to create CoreCLR, HRESULT:
//! 0x8007000C` while the report says the disk has room: the cause is memory
//! and swap, and a machine in that state has already pushed everything it
//! can into the compressor and onto disk. So the reader answers total
//! memory, obtainable memory, swap used and swap total, the compressor and
//! the lifetime swapouts, on both platforms.

use super::*;

/// Memory and swap, read from the host's own kernel.
///
/// Two fixed, read-only readers, one per platform, and a host missing either
/// reports what it has. On macOS available memory is the kernel's own
/// `kern.memorystatus_level` share of `hw.memsize`, the same figure the
/// host's capacity publication reads
/// ([`crate::providers::local::host_memory::reading`]).
pub const MEMORY_SECTION: &str = r#"if [ -x /usr/bin/vm_stat ]; then
  /usr/bin/vm_stat 2>/dev/null | while IFS= read -r row; do
    case "$row" in
      Mach*page\ size\ of*) printf 'STADO_MEMORY\tpage_size\t%s\n' "${row##*page size of }" ;;
      Pages\ occupied\ by\ compressor:*|Swapouts:*)
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

/// The `memory` block of the report: what the host's kernel says it has.
/// Readings only; no watermark is applied and nothing reclaims memory.
pub fn memory_report(reading: &DiskReading) -> Value {
    let memory = &reading.memory;
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
    })
}

/// The obtainable memory in kibibytes, as the first version of this report
/// spelled it: a decimal string, or absent when the host did not answer.
pub(super) fn available_kb(memory: &MemoryReading) -> Option<String> {
    let kib = crate::providers::local::host_memory::constants::MIB / 1024;
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

/// One decimal integer from a marker field.
pub(super) fn fold_int(value: &str) -> Option<i64> {
    value.trim().trim_end_matches('.').parse::<i64>().ok()
}

/// Turn the platform rows into the same figures the host's own pass reads.
///
/// The parsers come from
/// [`crate::providers::local::host_memory::reading`] so that a report about
/// a host and that host's own watermark verdict cannot be computed two
/// different ways.
pub(super) fn fold_memory(
    reading: &mut DiskReading,
    level_pct: Option<i64>,
    pages: &[String],
    meminfo: &[String],
) {
    use crate::providers::local::host_memory::reading as host;
    if !meminfo.is_empty() {
        let text = meminfo.join("\n");
        reading.memory.available_bytes = host::meminfo_bytes(&text, "MemAvailable");
        reading.memory.total_bytes = host::meminfo_bytes(&text, "MemTotal");
        let swap_total = host::meminfo_bytes(&text, "SwapTotal");
        let swap_free = host::meminfo_bytes(&text, "SwapFree");
        reading.memory.swap_total_bytes = swap_total;
        reading.memory.swap_used_bytes = match (swap_total, swap_free) {
            (Some(total), Some(free)) => Some(total.saturating_sub(free)),
            _ => None,
        };
        return;
    }
    reading.memory.available_bytes = level_pct
        .zip(reading.memory.total_bytes)
        .and_then(|(level, total)| host::macos_available_bytes(level, total));
    if pages.is_empty() {
        return;
    }
    let text = pages.join("\n");
    reading.memory.compressor_pages = host::vm_stat_pages(&text, host::MACOS_COMPRESSOR_ROW);
    reading.memory.swapouts = host::vm_stat_pages(&text, host::MACOS_SWAPOUT_ROW);
}
