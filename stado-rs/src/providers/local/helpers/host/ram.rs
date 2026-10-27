//! Host RAM as the operating system reports it: available and total memory,
//! or the operating system's own reason it could not answer. Nothing here
//! estimates.
//!
//! The figures are the host memory pass's own
//! ([`crate::providers::local::host_memory::reading::read_available_and_total`]):
//! one definition of "available" for admission and for the memory watermark,
//! so the agent cannot admit a job against memory the pass counts as taken.

use crate::providers::local::host_memory::constants::GIB;
use crate::providers::local::host_memory::reading::read_available_and_total;

/// Available and total host RAM in GiB, measured from the operating system,
/// or why the operating system did not answer.
pub fn memory_gb() -> Result<(f64, f64), String> {
    let (available, total) = read_available_and_total()?;
    let gib = |bytes: i64| bytes as f64 / GIB as f64;
    Ok((gib(available), gib(total)))
}
