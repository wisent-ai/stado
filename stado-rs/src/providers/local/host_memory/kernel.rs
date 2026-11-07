//! The one reader of an integer the macOS kernel states by name
//! (`sysctlbyname`), asked by this process: no `/usr/sbin/sysctl` runs, so
//! an agent session, a sandbox or a host-exec allowlist that refuses other
//! programs still gets the figure, and a reading costs no process start.
//! Every integer this host's memory readings take from the kernel is read
//! here, so the capacity publication and `stado host processes` cannot
//! answer differently.

/// The integer the kernel states under `name`: an `int` or a 64-bit value,
/// told apart by the size the kernel wrote.
#[cfg(target_os = "macos")]
pub(super) fn sysctl(name: &str) -> Result<i64, String> {
    use std::mem::size_of;

    use nix::errno::Errno;
    use nix::libc;

    let key = std::ffi::CString::new(name)
        .map_err(|error| format!("the sysctl name {name:?} holds a NUL byte: {error}"))?;
    let mut value = [0_u8; size_of::<i64>()];
    let mut size = value.len();
    // SAFETY: `key` is NUL-terminated, `value` has `size` writable bytes and
    // the kernel writes at most `size` of them, storing how many in `size`.
    let status = unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            value.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return Err(format!("sysctl {name} was refused: {}", Errno::last()));
    }
    if size == size_of::<i32>() {
        let mut narrow = [0_u8; size_of::<i32>()];
        narrow.copy_from_slice(&value[..size_of::<i32>()]);
        return Ok(i64::from(i32::from_ne_bytes(narrow)));
    }
    if size == size_of::<i64>() {
        return Ok(i64::from_ne_bytes(value));
    }
    Err(format!(
        "sysctl {name} answered {size} byte(s), neither an int nor a 64-bit integer"
    ))
}

/// Off macOS no kernel states its memory by sysctl name; the readers take
/// `/proc/meminfo` there and never ask this.
#[cfg(not(target_os = "macos"))]
pub(super) fn sysctl(name: &str) -> Result<i64, String> {
    Err(format!(
        "sysctl {name} is a macOS kernel reading, and this host's kernel states its memory in \
         /proc/meminfo"
    ))
}
