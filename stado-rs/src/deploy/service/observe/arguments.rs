//! Kernel arguments for local process lifecycle decisions. Rendered `ps`
//! command lines cannot distinguish an argument from words inside its value.
//! The same kernel record carries the process's environment, from which
//! [`process_home`] reads the home it runs under.

pub(crate) fn process_arguments(pid: u32) -> Result<Vec<String>, String> {
    read_record(pid)
        .map(|record| record.arguments)
        .map_err(|error| format!("reading argument vector for PID {pid}: {error}"))
}

/// The `HOME` process `pid` was started with, `None` when its environment
/// names none. A process this account cannot read (another user's) is the
/// error, never read as homeless.
pub(crate) fn process_home(pid: u32) -> Result<Option<std::path::PathBuf>, String> {
    let record =
        read_record(pid).map_err(|error| format!("reading the environment of PID {pid}: {error}"))?;
    Ok(record
        .environment
        .iter()
        .find_map(|entry| entry.strip_prefix("HOME="))
        .filter(|home| !home.is_empty())
        .map(std::path::PathBuf::from))
}

/// One process's argument vector and environment, as the kernel holds them.
struct Record {
    arguments: Vec<String>,
    environment: Vec<String>,
}

fn argument(bytes: &[u8]) -> Result<String, String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|error| format!("kernel argument is not UTF-8: {}", error.utf8_error()))
}

/// An environment entry; one that is not UTF-8 is kept lossily, since an
/// entry this reader does not look for must not refuse the read of the one
/// it does.
fn variable(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(target_os = "linux")]
fn read_record(pid: u32) -> Result<Record, String> {
    let path = format!("/proc/{pid}/cmdline");
    let bytes = std::fs::read(&path).map_err(|error| format!("{path}: {error}"))?;
    let arguments = bytes.strip_suffix(&[0]).ok_or_else(|| {
        format!("{path} contains no complete argument vector; refusing a lifecycle decision")
    })?;
    let arguments = arguments
        .split(|byte| *byte == 0)
        .map(argument)
        .collect::<Result<_, _>>()?;
    let path = format!("/proc/{pid}/environ");
    let environment = std::fs::read(&path).map_err(|error| format!("{path}: {error}"))?;
    Ok(Record {
        arguments,
        environment: environment
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .map(variable)
            .collect(),
    })
}

#[cfg(target_os = "macos")]
fn read_record(pid: u32) -> Result<Record, String> {
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
    // The environment follows the arguments, NUL-terminated strings ending at
    // the first empty one or at the end of what the kernel returned.
    let mut environment = Vec::new();
    while let Some(end) = remaining.iter().position(|byte| *byte == 0) {
        if end == 0 {
            break;
        }
        environment.push(variable(&remaining[..end]));
        remaining = &remaining[end + 1..];
    }
    Ok(Record {
        arguments,
        environment,
    })
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
fn read_record(_pid: u32) -> Result<Record, String> {
    Err("native process arguments are unsupported on this operating system".to_string())
}
