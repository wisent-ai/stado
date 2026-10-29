//! Process names on macOS: libproc's process list (`proc_listallpids`) and
//! each process's executable name (`proc_name`).

use nix::errno::Errno;
use nix::libc::{self, c_int, c_void};

/// The longest name `proc_name` writes: the kernel's `MAXCOMLEN` doubled,
/// as `<sys/proc_info.h>` sizes `proc_bsdinfo.pbi_name`.
const NAME_CAPACITY: usize = 2 * libc::MAXCOMLEN;

/// The executable name of every process the table lists. A process that
/// ends between the listing and the read is skipped; a listing the kernel
/// refuses is the error, so an unreadable table is never read as no CI.
pub(super) fn process_names() -> Result<Vec<String>, String> {
    let pids = pids()?;
    let capacity = u32::try_from(NAME_CAPACITY).map_err(|error| {
        format!("a process name buffer of {NAME_CAPACITY} bytes is too large: {error}")
    })?;
    let mut names = Vec::with_capacity(pids.len());
    for pid in pids {
        let mut buffer = [0u8; NAME_CAPACITY];
        // SAFETY: the buffer holds NAME_CAPACITY bytes and its size is passed.
        let written =
            unsafe { libc::proc_name(pid, buffer.as_mut_ptr().cast::<c_void>(), capacity) };
        // A process that ended since the listing has no name to read.
        let Ok(length) = usize::try_from(written) else {
            continue;
        };
        if length == 0 {
            continue;
        }
        names.push(String::from_utf8_lossy(&buffer[..length.min(NAME_CAPACITY)]).into_owned());
    }
    Ok(names)
}

/// Every process id the kernel lists. The size the kernel states first can
/// grow before the list is read, so a list that fills its buffer is read
/// again at the size the kernel states then.
fn pids() -> Result<Vec<c_int>, String> {
    loop {
        // SAFETY: a null buffer of size zero asks only for the count.
        let stated = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if stated < 0 {
            return Err(format!(
                "proc_listallpids could not count processes: {}",
                Errno::last()
            ));
        }
        let capacity = usize::try_from(stated).unwrap_or_default() + 1;
        let mut pids: Vec<c_int> = vec![0; capacity];
        let bytes = c_int::try_from(capacity * std::mem::size_of::<c_int>()).map_err(|error| {
            format!("a process list of {capacity} entries is too large: {error}")
        })?;
        // SAFETY: the buffer holds `capacity` process ids and its byte size is passed.
        let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), bytes) };
        if listed < 0 {
            return Err(format!(
                "proc_listallpids could not list processes: {}",
                Errno::last()
            ));
        }
        let listed = usize::try_from(listed).unwrap_or_default();
        if listed < capacity {
            pids.truncate(listed);
            return Ok(pids);
        }
    }
}
