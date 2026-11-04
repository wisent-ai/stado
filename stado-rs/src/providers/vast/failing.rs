//! Whether this host is failing in a way a renter would feel, read before
//! each listing and each poll while listed.
//!
//! A contract cannot be ended early, and a machine that crashes, drops off
//! the network or performs badly under one loses reliability score. The
//! signals are the ones this host already measures: the boards' driver
//! answering (`nvidia-smi`, the same read the host-hours register samples),
//! and the free space of the volume the beacon reports. A host failing
//! either withdraws its offer at once, so no new contract starts on it, and
//! announces maintenance for the contract already running.

/// The free space below which a renter's container cannot be given its disk.
pub const MIN_FREE_GIB: i64 = 20;

/// Why this host should not be rented now, or `None` when nothing it
/// measures is failing.
pub async fn failing() -> Option<String> {
    let mut causes = Vec::new();
    match crate::wait::output_async(
        tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=uuid", "--format=csv,noheader"]),
    )
    .await
    {
        Ok(output) if output.status.success() => {
            if String::from_utf8_lossy(&output.stdout).trim().is_empty() {
                causes.push("the NVIDIA driver reports no board".to_string());
            }
        }
        Ok(output) => causes.push(format!(
            "the NVIDIA driver failed (nvidia-smi exited {}: {})",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => causes.push(format!("nvidia-smi could not run: {error}")),
    }
    let home = crate::config_file::expand_tilde("~");
    match crate::providers::local::space_reclaim::free_bytes(&home) {
        Ok(free) => {
            let free_gib = free / crate::providers::local::host_memory::constants::GIB;
            if free_gib < MIN_FREE_GIB {
                causes.push(format!(
                    "{free_gib} GiB free on {}, below the {MIN_FREE_GIB} GiB a renter's disk needs",
                    home.display()
                ));
            }
        }
        Err(error) => causes.push(format!(
            "free space on {} cannot be read: {error}",
            home.display()
        )),
    }
    (!causes.is_empty()).then(|| causes.join("; "))
}
