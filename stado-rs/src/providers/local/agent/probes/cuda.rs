//! CUDA child probe.

use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::super::CUDA_PROBE_CACHE_S;

struct CudaProbe {
    checked_at: Instant,
    ok: bool,
    detail: String,
}

static CUDA_PROBE: LazyLock<Mutex<Option<CudaProbe>>> = LazyLock::new(|| Mutex::new(None));

/// Check that the host's native NVIDIA management interface can enumerate a
/// GPU. Workload-specific CUDA frameworks are validated by the workload
/// itself; the global agent must not import Python or `wisent` before claiming
/// an unrelated shell, native, or container job.
pub async fn gpu_driver_available() -> (bool, String) {
    if let Some(probe) = &*CUDA_PROBE.lock().expect("cuda probe cache lock") {
        if probe.checked_at.elapsed() < Duration::from_secs(CUDA_PROBE_CACHE_S) {
            return (probe.ok, probe.detail.clone());
        }
    }
    let (ok, detail) = run_cuda_probe().await;
    *CUDA_PROBE.lock().expect("cuda probe cache lock") = Some(CudaProbe {
        checked_at: Instant::now(),
        ok,
        detail: detail.clone(),
    });
    (ok, detail)
}

async fn run_cuda_probe() -> (bool, String) {
    let res = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=uuid", "--format=csv,noheader,nounits"])
            .output(),
    )
    .await;
    match res {
        Ok(Ok(out)) => cuda_probe_result(
            out.status.code().unwrap_or(-1),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        ),
        Ok(Err(exc)) => (false, format!("cuda probe raised: {exc}")),
        Err(_) => (false, "cuda probe raised: timed out after 30s".to_string()),
    }
}

/// Pure: `(ok, detail)` from one native driver probe. Detail is truncated to
/// a bounded suffix for capacity diagnostics.
pub fn cuda_probe_result(rc: i32, stdout: &str, stderr: &str) -> (bool, String) {
    let raw = if !stdout.is_empty() {
        stdout.to_string()
    } else if !stderr.is_empty() {
        stderr.to_string()
    } else {
        format!("rc={rc}")
    };
    let trimmed = raw.trim();
    let detail: String = trimmed
        .chars()
        .rev()
        .take(300)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    (rc == 0, detail)
}
