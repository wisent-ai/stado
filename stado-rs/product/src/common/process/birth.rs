//! A process's birth time, read from the kernel.
//!
//! A PID alone can be reused before a later install reads its record, so a
//! lock or an after-install marker records its holder's birth time with it.
//! The kernel is read by the wait crate's one reader
//! ([`stado_wait::open::birth_seconds`]), the one that also reads every wait
//! and holding of the register against its process's birth.
use anyhow::{anyhow, Context, Result};

/// The operating system's birth time for a PID, or absence when it has
/// exited. The text is what `ps -o lstart=` prints, in local time, so a birth
/// time an earlier Stado recorded keeps comparing equal.
pub fn started_at(pid: i32) -> Result<Option<String>> {
    let Some(seconds) = stado_wait::open::birth_seconds(pid).map_err(|cause| anyhow!(cause))?
    else {
        return Ok(None);
    };
    let born = std::time::UNIX_EPOCH
        .checked_add(std::time::Duration::from_secs(seconds))
        .with_context(|| {
            format!("pid {pid} reports birth second {seconds}, past this host's clock")
        })?;
    Ok(Some(
        chrono::DateTime::<chrono::Local>::from(born)
            .format("%a %b %e %H:%M:%S %Y")
            .to_string(),
    ))
}
