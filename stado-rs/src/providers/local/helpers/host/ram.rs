//! Host RAM: what the operating system reports, what other software already
//! holds, and how much of it stays reserved.
//!
//! Nothing here estimates. The baseline reserve is walked out of the process
//! table so an unrelated tenant (ComfyUI, a system daemon) is subtracted as
//! the size it actually is, and the per-slot footprint is read the same way.

use std::path::Path;

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
fn linux_memory_gb() -> Result<(f64, f64), String> {
    let path = "/proc/meminfo";
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    let field = |key: &str| {
        parse_meminfo_gb(&text, key).ok_or_else(|| format!("{path} carries no readable {key}"))
    };
    Ok((field("MemAvailable:")?, field("MemTotal:")?))
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn macos_memory_gb() -> Result<(f64, f64), String> {
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
        return Err(format!(
            "host_statistics64 failed with kern_return_t {status}"
        ));
    }
    let page_size = unsafe { nix::libc::vm_page_size } as f64;
    if page_size <= 0.0 {
        return Err(format!("vm_page_size is {page_size}"));
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
        return Err(format!(
            "sysctlbyname hw.memsize failed: status {total_status}, {total_size} bytes written ({})",
            std::io::Error::last_os_error()
        ));
    }
    const GIB: f64 = (1024_u64 * 1024 * 1024) as f64;
    Ok((
        available_pages as f64 * page_size / GIB,
        total_bytes as f64 / GIB,
    ))
}

/// Available and total host RAM in GiB, measured from the operating system,
/// or why the operating system did not answer.
pub fn memory_gb() -> Result<(f64, f64), String> {
    #[cfg(target_os = "macos")]
    {
        macos_memory_gb()
    }
    #[cfg(not(target_os = "macos"))]
    {
        linux_memory_gb()
    }
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
