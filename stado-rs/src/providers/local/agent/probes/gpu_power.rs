//! The host-level NVIDIA board power cap the registry declares.

use std::time::Duration;

async fn read_gpu_power_limits() -> Result<Vec<f64>, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new("nvidia-smi")
            .args(["--query-gpu=power.limit", "--format=csv,noheader,nounits"])
            .output(),
    )
    .await
    .map_err(|_| "nvidia-smi power query timed out after 30s".to_string())?
    .map_err(|error| format!("nvidia-smi power query failed: {error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "nvidia-smi power query exited {}: {}",
            output.status.code().unwrap_or(-1),
            detail.trim()
        ));
    }
    let limits = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.parse::<f64>()
                .map_err(|error| format!("invalid nvidia-smi power limit {line:?}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if limits.is_empty() {
        return Err("nvidia-smi power query returned no GPUs".to_string());
    }
    Ok(limits)
}

/// Reconcile the host-level NVIDIA board power cap declared in the registry.
/// Existing jobs keep running on failure, but the caller publishes zero free
/// capacity until the driver accepts and reports the declared limit.
pub async fn reconcile_gpu_power_limit(watts: u32) -> Result<String, String> {
    let desired = f64::from(watts);
    let current = read_gpu_power_limits().await?;
    if !current.iter().all(|actual| (actual - desired).abs() < 0.5) {
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            tokio::process::Command::new("nvidia-smi")
                .arg(format!("--power-limit={watts}"))
                .output(),
        )
        .await
        .map_err(|_| "nvidia-smi power-limit update timed out after 30s".to_string())?
        .map_err(|error| format!("nvidia-smi power-limit update failed: {error}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "nvidia-smi power-limit update exited {}: {}",
                output.status.code().unwrap_or(-1),
                detail.trim()
            ));
        }
    }
    let actual = read_gpu_power_limits().await?;
    if !actual.iter().all(|value| (*value - desired).abs() < 0.5) {
        return Err(format!(
            "driver reported power limits {actual:?} W after requesting {watts} W"
        ));
    }
    Ok(format!("{actual:?} W"))
}
