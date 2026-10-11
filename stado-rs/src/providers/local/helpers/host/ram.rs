//! Host RAM as the operating system reports it: available and total memory,
//! or the operating system's own reason it could not answer. Nothing here
//! estimates.

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
