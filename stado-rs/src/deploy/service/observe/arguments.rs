//! Kernel arguments for local process lifecycle decisions. Rendered `ps`
//! command lines cannot distinguish an argument from words inside its value.

pub(crate) fn process_arguments(pid: u32) -> Result<Vec<String>, String> {
    read_arguments(pid).map_err(|error| format!("reading argument vector for PID {pid}: {error}"))
}

fn argument(bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|error| format!("kernel argument is not UTF-8: {}", error.utf8_error()))
}

#[cfg(target_os = "linux")]
fn read_arguments(pid: u32) -> Result<Vec<String>, String> {
    let path = format!("/proc/{pid}/cmdline");
    let bytes = std::fs::read(&path).map_err(|error| format!("{path}: {error}"))?;
    let arguments = bytes.strip_suffix(&[0]).ok_or_else(|| {
        format!("{path} contains no complete argument vector; refusing a lifecycle decision")
    })?;
    arguments.split(|byte| *byte == 0).map(argument).collect()
}

#[cfg(target_os = "macos")]
fn read_arguments(pid: u32) -> Result<Vec<String>, String> {
    use nix::libc;
    use std::mem::size_of;
    use std::ptr;

    let pid = libc::c_int::try_from(pid).map_err(|error| format!("invalid PID: {error}"))?;
    let mut maximum: libc::c_int = 0;
    let mut size = size_of::<libc::c_int>();
    let mut limit_query = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: both arrays and the integer output remain live for this synchronous
    // query. The output length describes precisely the writable integer.
    let result = unsafe {
        libc::sysctl(
            limit_query.as_mut_ptr(),
            limit_query.len() as libc::c_uint,
            (&mut maximum as *mut libc::c_int).cast(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return Err(format!(
            "sysctl(KERN_ARGMAX): {}",
            std::io::Error::last_os_error()
        ));
    }
    if size != size_of::<libc::c_int>() || maximum <= 0 {
        return Err("sysctl(KERN_ARGMAX) returned an invalid argument-buffer size".to_string());
    }
    let mut bytes = vec![0u8; maximum as usize];
    size = bytes.len();
    let mut query = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: the kernel may write at most `size` bytes into this initialized
    // buffer. This read-only query supplies no new-value pointer or length.
    let result = unsafe {
        libc::sysctl(
            query.as_mut_ptr(),
            query.len() as libc::c_uint,
            bytes.as_mut_ptr().cast(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return Err(format!(
            "sysctl(KERN_PROCARGS2): {}",
            std::io::Error::last_os_error()
        ));
    }
    if size > bytes.len() {
        return Err("sysctl(KERN_PROCARGS2) exceeded its supplied argument buffer".to_string());
    }
    bytes.truncate(size);
    let header = bytes
        .get(..size_of::<libc::c_int>())
        .ok_or("sysctl(KERN_PROCARGS2) omitted argc")?;
    let count = libc::c_int::from_ne_bytes(header.try_into().map_err(|_| "invalid argc header")?);
    if count <= 0 {
        return Err("sysctl(KERN_PROCARGS2) returned an empty argument vector".to_string());
    }
    let mut remaining = &bytes[header.len()..];
    // Darwin returns argc, a terminated executable path, alignment padding,
    // then exactly argc terminated arguments. Environment strings follow them.
    take_string(&mut remaining)?;
    while remaining.first() == Some(&0) {
        remaining = &remaining[1..];
    }
    let mut arguments = Vec::new();
    for _ in 0..count {
        arguments.push(argument(take_string(&mut remaining)?)?);
    }
    Ok(arguments)
}

#[cfg(target_os = "macos")]
fn take_string<'a>(remaining: &mut &'a [u8]) -> Result<&'a [u8], String> {
    let end = remaining
        .iter()
        .position(|byte| *byte == 0)
        .ok_or("sysctl(KERN_PROCARGS2) returned a truncated string")?;
    let value = &remaining[..end];
    *remaining = &remaining[end + 1..];
    Ok(value)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_arguments(_pid: u32) -> Result<Vec<String>, String> {
    Err("native process arguments are unsupported on this operating system".to_string())
}
