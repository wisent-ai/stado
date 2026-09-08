//! Host RAM: what the operating system reports, what other software already
//! holds, and how much of it stays reserved.
//!
//! Nothing here estimates. The baseline reserve is walked out of the process
//! table so an unrelated tenant (ComfyUI, a system daemon) is subtracted as
//! the size it actually is, and the per-slot footprint is read the same way.

use std::path::Path;

use crate::constants;

/// Pure parser for /proc/meminfo: value of `key` (e.g. "MemAvailable:")
/// in GB. None when the key is absent or unparsable.
pub fn parse_meminfo_gb(text: &str, key: &str) -> Option<f64> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb as f64 / (1024.0 * 1024.0));
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn linux_memory_gb() -> Option<(f64, f64)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    Some((
        parse_meminfo_gb(&text, "MemAvailable:")?,
        parse_meminfo_gb(&text, "MemTotal:")?,
    ))
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn macos_memory_gb() -> Option<(f64, f64)> {
    let mut stats: nix::libc::vm_statistics64_data_t = unsafe { std::mem::zeroed() };
    let mut count = nix::libc::HOST_VM_INFO64_COUNT;
    // SAFETY: both pointers reference initialized, correctly sized writable
    // storage and Mach writes at most HOST_VM_INFO64_COUNT integer words.
    let status = unsafe {
        nix::libc::host_statistics64(
            nix::libc::mach_host_self(),
            nix::libc::HOST_VM_INFO64,
            (&mut stats as *mut nix::libc::vm_statistics64_data_t).cast::<nix::libc::integer_t>(),
            &mut count,
        )
    };
    if status != nix::libc::KERN_SUCCESS {
        return None;
    }
    let page_size = unsafe { nix::libc::vm_page_size } as f64;
    if page_size <= 0.0 {
        return None;
    }
    let available_pages = u64::from(stats.free_count)
        + u64::from(stats.inactive_count)
        + u64::from(stats.speculative_count)
        + u64::from(stats.purgeable_count);

    let mut total_bytes = 0_u64;
    let mut total_size = std::mem::size_of::<u64>();
    // SAFETY: the C string is NUL-terminated and sysctlbyname writes at most
    // the supplied u64-sized buffer.
    let total_status = unsafe {
        nix::libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut total_bytes as *mut u64).cast(),
            &mut total_size,
            std::ptr::null_mut(),
            0,
        )
    };
    if total_status != 0 || total_size != std::mem::size_of::<u64>() {
        return None;
    }
    const GIB: f64 = (1024_u64 * 1024 * 1024) as f64;
    Some((
        available_pages as f64 * page_size / GIB,
        total_bytes as f64 / GIB,
    ))
}

/// Available and total host RAM in GiB, measured from the operating system.
pub fn memory_gb() -> Option<(f64, f64)> {
    #[cfg(target_os = "macos")]
    {
        macos_memory_gb()
    }
    #[cfg(not(target_os = "macos"))]
    {
        linux_memory_gb()
    }
}

/// Free host RAM in GiB; `-1.0` means the operating system did not answer.
pub fn free_ram_gb() -> f64 {
    memory_gb().map(|(free, _)| free).unwrap_or(-1.0)
}

/// Total host RAM in GiB; `-1.0` means the operating system did not answer.
pub fn total_ram_gb() -> f64 {
    memory_gb().map(|(_, total)| total).unwrap_or(-1.0)
}

/// Pure parser for /proc/<pid>/status: first VmRSS value in kB.
pub fn parse_status_vmrss_kb(text: &str) -> Option<u64> {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// Sum of non-wisent, non-slot resident RAM (GB). Python
/// `_static_ram_reserve_gb`.
///
/// Walks /proc/*/status and adds VmRSS for every process that is NOT the
/// agent itself and does not look like an extraction slot
/// (extract_and_upload, upload_worker) or the agent binary. This captures
/// ComfyUI, system daemons, and any other baseline load without hardcoding
/// a reserve number.
pub fn static_ram_reserve_gb() -> f64 {
    static_ram_reserve_gb_at(Path::new("/proc"))
}

/// [`static_ram_reserve_gb`] with an injectable procfs root (tests use a
/// fabricated tree under a TempDir).
pub fn static_ram_reserve_gb_at(proc_root: &Path) -> f64 {
    let own = std::process::id();
    let mut total_kb: u64 = 0;
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return 0.0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if pid == own {
            continue;
        }
        let Ok(cmd) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let cmd: Vec<u8> = cmd
            .iter()
            .map(|b| if *b == 0 { b' ' } else { *b })
            .collect();
        // Skip the agent binary, extraction slots, and their upload workers.
        if [
            b"wc agent".as_slice(),
            b"extract_and_upload",
            b"upload_worker",
        ]
        .iter()
        .any(|token| contains_subslice(&cmd, token))
        {
            continue;
        }
        if let Some(kb) = std::fs::read_to_string(entry.path().join("status"))
            .ok()
            .and_then(|text| parse_status_vmrss_kb(&text))
        {
            total_kb += kb;
        }
    }
    total_kb as f64 / (1024.0 * 1024.0)
}

/// Dynamic RAM headroom:
/// 5% of total RAM with a 4 GiB floor.
/// Python `_ram_safety_buffer_gb`.
pub fn ram_safety_buffer_gb() -> f64 {
    let total = total_ram_gb();
    if total <= 0.0 {
        return constants::RAM_SAFETY_BUFFER_MIN_GB as f64;
    }
    (constants::RAM_SAFETY_BUFFER_MIN_GB as f64).max(total * constants::RAM_SAFETY_BUFFER_FRACTION)
}

/// Pure: summed VmRSS of `pids` under `proc_root`, in GB.
pub fn sum_rss_gb(proc_root: &Path, pids: &std::collections::HashSet<i32>) -> f64 {
    let mut total_kb: u64 = 0;
    for p in pids {
        if let Some(kb) = std::fs::read_to_string(proc_root.join(p.to_string()).join("status"))
            .ok()
            .and_then(|text| parse_status_vmrss_kb(&text))
        {
            total_kb += kb;
        }
    }
    total_kb as f64 / (1024.0 * 1024.0)
}
