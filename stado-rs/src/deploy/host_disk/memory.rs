//! The memory half of `stado space report TARGET`: what the host has left,
//! what its registry declaration says about it, and what its last
//! memory-reclaim pass did.
//!
//! Until 2026-09-06 this file could read only free pages and the swap line,
//! and only on macOS. `stado runner install charless-mac-mini --profile
//! precheck` failed four times with `Failed to create CoreCLR, HRESULT:
//! 0x8007000C` while the report said 5.7 GiB of disk was free and said
//! nothing at all about the 638 MiB of memory and the 4.7 GiB of swap that
//! were the actual cause. Free pages alone cannot answer that: the machine
//! had already pushed everything it could into the compressor and onto disk.
//!
//! So the reader now answers what a watermark can be measured against — total
//! memory, obtainable memory, swap used and swap total, the compressor and
//! the lifetime swapouts — on both platforms, and reads the memory pass's own
//! state file the same way the disk half reads the janitor's.

use super::*;

/// Memory and swap, read from the host's own kernel.
///
/// Two fixed, read-only readers, one per platform, and a host missing either
/// reports what it has. The macOS sum is free plus speculative plus purgeable
/// pages — what an allocation can obtain without evicting anonymous memory —
/// because macOS publishes no `MemAvailable`.
pub const MEMORY_SECTION: &str = r#"if [ -x /usr/bin/vm_stat ]; then
  /usr/bin/vm_stat 2>/dev/null | while IFS= read -r row; do
    case "$row" in
      Mach*page\ size\ of*) printf 'STADO_MEMORY\tpage_size\t%s\n' "${row##*page size of }" ;;
      Pages\ free:*|Pages\ speculative:*|Pages\ purgeable:*|Pages\ occupied\ by\ compressor:*|Swapouts:*)
        printf 'STADO_MEMORY_PAGES\t%s\n' "$row" ;;
    esac
  done
fi
if [ -x /usr/sbin/sysctl ]; then
  printf 'STADO_MEMORY\ttotal_bytes\t%s\n' "$(/usr/sbin/sysctl -n hw.memsize 2>/dev/null)"
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

/// The memory pass's own state file — the `memory_reclaim` field.
pub const MEMORY_STATE_SECTION: &str = r#"memory_state="$HOME/@MEMORY_STATE_PATH@"
if [ -r "$memory_state" ]; then
  printf 'STADO_MEMORY_STATE\t%s\n' "$(/usr/bin/tr -d '\t\r\n' < "$memory_state")"
else
  printf 'STADO_MEMORY_STATE_MISSING\t%s\n' "$memory_state"
fi
"#;

/// The `memory` block of the report: the host's reading beside the
/// declaration it is measured against and the last pass that measured it.
pub fn memory_report(target: &ComputeTarget, reading: &DiskReading) -> Value {
    let declared = crate::providers::local::host_memory::schema::declared(target);
    let policy = declared.clone().unwrap_or_else(
        crate::providers::local::host_memory::MemoryReclaimPolicy::reporting_default,
    );
    let memory = &reading.memory;
    let over_low = match memory.available_bytes {
        Some(available) => Value::Bool(available < policy.low_free_bytes()),
        None => Value::Null,
    };
    let over_swap = match memory.swap_used_pct() {
        Some(pct) => Value::Bool(pct >= policy.high_swap_used_pct),
        None => Value::Null,
    };
    json!({
        "reading": {
            "available_bytes": memory.available_bytes,
            "available_mb": memory.available_mb(),
            "total_bytes": memory.total_bytes,
            "swap_used_bytes": memory.swap_used_bytes,
            "swap_total_bytes": memory.swap_total_bytes,
            "swap_used_pct": memory.swap_used_pct(),
            "compressor_pages": memory.compressor_pages,
            "swapouts": memory.swapouts,
        },
        "declaration": {
            "declared": declared.is_some(),
            "source": "registry targets[].memory_reclaim",
            "policy": serde_json::to_value(&policy).unwrap_or(Value::Null),
        },
        "watermarks": {
            "low_watermark_bytes": policy.low_free_bytes(),
            "target_watermark_bytes": policy.target_free_bytes(),
            "high_swap_used_pct": policy.high_swap_used_pct,
            "below_low_watermark": over_low,
            "over_swap_watermark": over_swap,
        },
        "last_pass": reading.memory_state.clone(),
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
    page_size: Option<i64>,
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
    if pages.is_empty() {
        return;
    }
    let text = pages.join("\n");
    reading.memory.compressor_pages = host::vm_stat_pages(&text, host::MACOS_COMPRESSOR_ROW);
    reading.memory.swapouts = host::vm_stat_pages(&text, host::MACOS_SWAPOUT_ROW);
    if let Some(page_size) = page_size {
        let obtainable: i64 = host::MACOS_OBTAINABLE_CLASSES
            .iter()
            .filter_map(|label| host::vm_stat_pages(&text, label))
            .sum();
        reading.memory.available_bytes = Some(obtainable.saturating_mul(page_size));
    }
}
