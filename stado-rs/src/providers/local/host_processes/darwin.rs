//! Process counters on macOS: libproc's process list, `proc_pidpath` and
//! `proc_pid_rusage`.

use std::mem::{size_of_val, MaybeUninit};
use std::num::{NonZeroU128, NonZeroUsize};
use std::time::Duration;

use nix::errno::Errno;
use nix::libc::{self, c_int};

use super::{Counters, DiskBytes, Reading};

/// One process's resource record, or the kernel's error for it.
fn rusage(pid: c_int) -> Result<libc::rusage_info_v4, Errno> {
    let mut info = MaybeUninit::<libc::rusage_info_v4>::uninit();
    // SAFETY: the buffer is one rusage_info_v4 and RUSAGE_INFO_V4 asks the
    // kernel for exactly that record.
    let status = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast::<libc::rusage_info_t>(),
        )
    };
    Errno::result(status)?;
    // SAFETY: the call succeeded, so the kernel filled the record.
    Ok(unsafe { info.assume_init() })
}

/// The unit `proc_pid_rusage` counts CPU time in, measured on this process.
///
/// The record counts CPU in Mach absolute-time units, which are nanoseconds
/// on Intel Macs and not on Apple silicon. Rather than assume either, this
/// process's own record is read beside `getrusage`, which reports the same
/// CPU time of the same process in seconds and microseconds; their ratio
/// converts every other process's record.
struct Clock {
    nanoseconds: u128,
    units: NonZeroU128,
}

impl Clock {
    fn measure() -> Result<Self, String> {
        let pid = c_int::try_from(std::process::id())
            .map_err(|error| format!("this process's id does not fit a pid: {error}"))?;
        let own = rusage(pid)
            .map_err(|error| format!("proc_pid_rusage of this process failed: {error}"))?;
        let mut usage = MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: the buffer is one rusage, which RUSAGE_SELF fills.
        Errno::result(unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) })
            .map_err(|error| format!("getrusage of this process failed: {error}"))?;
        // SAFETY: the call succeeded, so the record is filled.
        let usage = unsafe { usage.assume_init() };
        let spent = timeval(usage.ru_utime)? + timeval(usage.ru_stime)?;
        let units = u128::from(own.ri_user_time) + u128::from(own.ri_system_time);
        let units = NonZeroU128::new(units).ok_or_else(|| {
            "this process's own resource record counts no CPU time yet, so the unit of every \
             process's CPU counter cannot be measured"
                .to_string()
        })?;
        Ok(Self {
            nanoseconds: spent.as_nanos(),
            units,
        })
    }

    fn nanoseconds(&self, units: u64) -> Result<u64, String> {
        u64::try_from(u128::from(units) * self.nanoseconds / self.units.get()).map_err(|error| {
            format!("a CPU counter of {units} units overflows nanoseconds: {error}")
        })
    }
}

fn timeval(value: libc::timeval) -> Result<Duration, String> {
    let seconds = u64::try_from(value.tv_sec)
        .map_err(|error| format!("getrusage reported negative seconds: {error}"))?;
    let micros = u64::try_from(value.tv_usec)
        .map_err(|error| format!("getrusage reported negative microseconds: {error}"))?;
    Ok(Duration::from_secs(seconds) + Duration::from_micros(micros))
}

/// Every pid the kernel lists. A call with no buffer answers how many pids
/// there are; the list is then asked for with room for them, and again with
/// more room whenever it filled every slot, since processes start between
/// the calls.
fn pids() -> Result<Vec<c_int>, String> {
    let mut pids: Vec<c_int> = Vec::new();
    loop {
        let spare = pids.spare_capacity_mut();
        let room = spare.len();
        let bytes = c_int::try_from(size_of_val(spare)).map_err(|error| {
            format!("a process list of {room} pids is too large to ask for: {error}")
        })?;
        let buffer = if spare.is_empty() {
            std::ptr::null_mut()
        } else {
            spare.as_mut_ptr().cast()
        };
        // SAFETY: `buffer` is null, asking for the count, or holds `bytes`
        // writable bytes of pids.
        let count = Errno::result(unsafe { libc::proc_listallpids(buffer, bytes) })
            .map_err(|error| format!("proc_listallpids failed: {error}"))?;
        let count = usize::try_from(count)
            .map_err(|error| format!("proc_listallpids returned {count}: {error}"))?;
        if !buffer.is_null() && count < room {
            // SAFETY: the kernel wrote `count` pids into the buffer.
            unsafe { pids.set_len(count) };
            return Ok(pids);
        }
        let more = NonZeroUsize::new(count)
            .ok_or_else(|| "proc_listallpids counts no process on this host".to_string())?;
        pids.reserve_exact(room + more.get());
    }
}

/// The executable path the kernel holds for `pid`.
fn program(pid: c_int) -> Result<String, Errno> {
    let mut path: Vec<u8> = Vec::with_capacity(libc::PROC_PIDPATHINFO_MAXSIZE as usize);
    let spare = path.spare_capacity_mut();
    let room = u32::try_from(spare.len()).map_err(|_too_large| Errno::EOVERFLOW)?;
    // SAFETY: the buffer has `room` writable bytes.
    let length = Errno::result(unsafe { libc::proc_pidpath(pid, spare.as_mut_ptr().cast(), room) })?;
    let length = usize::try_from(length).map_err(|_negative| Errno::EOVERFLOW)?;
    // SAFETY: the kernel wrote `length` bytes of the path.
    unsafe { path.set_len(length) };
    Ok(String::from_utf8_lossy(&path).into_owned())
}

/// Read every listed process. A process that ended between the list and
/// its read (ESRCH) has no counters left and is not in the reading; any
/// other refusal is kept by pid with the kernel's error.
pub(super) fn read() -> Result<Reading, String> {
    let clock = Clock::measure()?;
    let mut reading = Reading::default();
    for pid in pids()? {
        let read = rusage(pid).and_then(|info| Ok((info, program(pid)?)));
        let (info, program) = match read {
            Ok(read) => read,
            Err(Errno::ESRCH) => continue,
            Err(error) => {
                reading.unreadable.insert(pid, error.desc().to_string());
                continue;
            }
        };
        let counters = Counters {
            program,
            started: Some(info.ri_proc_start_abstime),
            cpu_ns: clock.nanoseconds(info.ri_user_time + info.ri_system_time)?,
            children_cpu_ns: Some(
                clock.nanoseconds(info.ri_child_user_time + info.ri_child_system_time)?,
            ),
            resident_bytes: info.ri_resident_size,
            disk: Ok(DiskBytes {
                read: info.ri_diskio_bytesread,
                written: info.ri_diskio_byteswritten,
            }),
        };
        reading.processes.insert(pid, counters);
    }
    Ok(reading)
}
