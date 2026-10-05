//! CUDA child probe.

/// Check that the host's native NVIDIA management interface can enumerate a
/// GPU. Workload-specific CUDA frameworks are validated by the workload
/// itself; the global agent must not import Python or `wisent` before claiming
/// an unrelated shell, native, or container job.
///
/// Every call asks the driver: the answer gates a claim on this tick, and a
/// driver that recovered or failed since the last tick is seen on this one.
pub async fn gpu_driver_available() -> (bool, String) {
    let res = tokio::process::Command::new("nvidia-smi")
        .args(["--query-gpu=uuid", "--format=csv,noheader,nounits"])
        .output()
        .await;
    match res {
        Ok(out) => cuda_probe_result(
            out.status.code().unwrap_or(-1),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Err(exc) => (false, format!("cuda probe raised: {exc}")),
    }
}

/// Pure: `(ok, detail)` from one native driver probe. The detail is the
/// probe's whole output, trimmed.
pub fn cuda_probe_result(rc: i32, stdout: &str, stderr: &str) -> (bool, String) {
    let raw = if !stdout.is_empty() {
        stdout
    } else if !stderr.is_empty() {
        stderr
    } else {
        return (rc == 0, format!("rc={rc}"));
    };
    (rc == 0, raw.trim().to_string())
}
