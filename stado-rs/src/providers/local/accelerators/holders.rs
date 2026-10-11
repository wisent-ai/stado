//! Who holds each accelerator: the driver's per-process rows, each mapped to
//! its unit and to the Stado job it belongs to, if any.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::providers::local::probe::gpu::proc_tree_pids;
use crate::providers::local::slots::ActiveSlot;

/// One process on one accelerator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcceleratorHolder {
    /// The card, as the driver names it (its UUID).
    pub device: String,
    pub pid: i32,
    pub process: String,
    pub used_vram_gb: f64,
    /// The systemd unit (Linux cgroup) the process runs under, when one does.
    pub unit: Option<String>,
    /// Why the process's cgroup could not be read, when it existed and could
    /// not be: the unit is then unknown, not absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_error: Option<String>,
    /// The Stado job whose process tree the pid belongs to, when one does.
    pub stado_job: Option<String>,
}

/// The whole reading, ready for the capacity publication's `diag`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AcceleratorHolders {
    pub holders: Vec<AcceleratorHolder>,
    /// `discrete` when the driver was asked, `unified` on Apple silicon
    /// where the GPU shares the host's memory and no per-process VRAM exists.
    pub memory_model: &'static str,
    /// Why the driver could not be asked, when it could not.
    pub error: Option<String>,
}

const MIB_PER_GIB: f64 = 1024.0;

/// Rows of `nvidia-smi --query-compute-apps=gpu_uuid,pid,process_name,used_memory
/// --format=csv,noheader,nounits`. The used memory is always the last
/// field, so a process name carrying a comma is everything between the pid
/// and it.
pub fn parse_compute_apps_named(text: &str) -> Vec<(String, i32, String, i64)> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').map(str::trim).collect();
            if parts.len() < 4 {
                return None;
            }
            let used: i64 = parts[parts.len() - 1].parse().ok()?;
            let pid: i32 = parts[1].parse().ok()?;
            Some((
                parts[0].to_string(),
                pid,
                parts[2..parts.len() - 1].join(","),
                used,
            ))
        })
        .collect()
}

/// The systemd unit a pid runs under, from its cgroup path; `None` outside
/// Linux, for a process that has exited, or when the cgroup names no unit.
/// A cgroup file that exists but cannot be read is an error naming it.
fn unit_of(pid: i32) -> Result<Option<String>, String> {
    let path = format!("/proc/{pid}/cgroup");
    let cgroup = match std::fs::read_to_string(&path) {
        Ok(cgroup) => cgroup,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {path}: {error}")),
    };
    Ok(cgroup
        .lines()
        .filter_map(|line| line.rsplit('/').next())
        .find(|segment| segment.ends_with(".service") || segment.ends_with(".scope"))
        .map(str::to_string))
}

fn unreadable(error: String) -> AcceleratorHolders {
    AcceleratorHolders {
        holders: Vec::new(),
        memory_model: "discrete",
        error: Some(error),
    }
}

/// Measure the holders. Apple silicon answers at once with the unified
/// model; anywhere else the driver is asked, and a driver that is absent or
/// refuses is reported as such rather than as an empty card.
pub async fn measure(slots: &[ActiveSlot]) -> AcceleratorHolders {
    if cfg!(target_os = "macos") {
        return AcceleratorHolders {
            holders: Vec::new(),
            memory_model: "unified",
            error: None,
        };
    }
    let output = match crate::wait::output_async(tokio::process::Command::new("nvidia-smi").args([
        "--query-compute-apps=gpu_uuid,pid,process_name,used_memory",
        "--format=csv,noheader,nounits",
    ]))
    .await
    {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return unreadable(format!(
                "nvidia-smi exited {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
        Err(error) => return unreadable(format!("nvidia-smi could not be run: {error}")),
    };
    let rows = parse_compute_apps_named(&String::from_utf8_lossy(&output.stdout));
    let mut trees: Vec<(String, HashSet<i32>)> = Vec::new();
    for slot in slots {
        if let Some(pid) = slot.slot.pid {
            trees.push((slot.slot.job.job_id.clone(), proc_tree_pids(pid).await));
        }
    }
    let holders = rows
        .into_iter()
        .map(|(device, pid, process, used_mib)| {
            let (unit, unit_error) = match unit_of(pid) {
                Ok(unit) => (unit, None),
                Err(error) => (None, Some(error)),
            };
            AcceleratorHolder {
                device,
                pid,
                process,
                used_vram_gb: used_mib as f64 / MIB_PER_GIB,
                unit,
                unit_error,
                stado_job: trees
                    .iter()
                    .find(|(_, pids)| pids.contains(&pid))
                    .map(|(job, _)| job.clone()),
            }
        })
        .collect();
    AcceleratorHolders {
        holders,
        memory_model: "discrete",
        error: None,
    }
}

impl AcceleratorHolders {
    /// Put the reading into a capacity publication's `diag`, beside the VRAM
    /// figures it explains: `vram_unattributed_gb` is the used memory no
    /// listed holder accounts for.
    pub fn insert_into(
        &self,
        diag: &mut Map<String, Value>,
        total_vram_gb: i64,
        free_vram_gb: i64,
    ) {
        let error = match serde_json::to_value(&self.holders) {
            Ok(holders) => {
                diag.insert("accelerator_holders".into(), holders);
                self.error.clone()
            }
            Err(error) => {
                diag.remove("accelerator_holders");
                Some(format!("cannot serialise the accelerator holders: {error}"))
            }
        };
        diag.insert(
            "accelerator_memory_model".into(),
            Value::from(self.memory_model),
        );
        match error {
            Some(error) => {
                diag.insert("accelerator_holders_error".into(), Value::from(error));
            }
            None => {
                diag.remove("accelerator_holders_error");
            }
        }
        if self.memory_model == "discrete" && total_vram_gb > 0 {
            let used = (total_vram_gb - free_vram_gb).max(0) as f64;
            let attributed: f64 = self.holders.iter().map(|holder| holder.used_vram_gb).sum();
            diag.insert(
                "vram_unattributed_gb".into(),
                Value::from((used - attributed).max(0.0)),
            );
        }
    }
}

/// One sentence for a report: who holds the accelerators, read from a
/// publication's `diag`. `None` when the publication carries no reading.
pub fn accelerator_holders_line(diag: &Map<String, Value>) -> Option<String> {
    if let Some(error) = diag
        .get("accelerator_holders_error")
        .and_then(Value::as_str)
    {
        return Some(format!("accelerator holders unknown: {error}"));
    }
    let model = diag
        .get("accelerator_memory_model")
        .and_then(Value::as_str)?;
    if model == "unified" {
        return Some("accelerator shares the host's memory; no per-process VRAM".to_string());
    }
    // `insert_into` writes the holders beside the memory model, so a
    // publication without them, or with a list that does not decode, says
    // nothing about who holds the card: reported, never read as "nobody".
    let holders: Vec<AcceleratorHolder> = match diag.get("accelerator_holders") {
        None => {
            return Some(
                "accelerator holders unknown: the publication names a memory model but carries no holders"
                    .to_string(),
            )
        }
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(holders) => holders,
            Err(error) => return Some(format!("accelerator holders unreadable: {error}")),
        },
    };
    let unattributed = diag
        .get("vram_unattributed_gb")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if holders.is_empty() {
        return Some(if unattributed > 0.0 {
            format!(
                "no process holds the accelerator through the driver, yet {unattributed:.1} GiB is in use"
            )
        } else {
            "no process holds the accelerator".to_string()
        });
    }
    let mut parts: Vec<String> = holders
        .iter()
        .map(|holder| {
            format!(
                "pid {} {} {:.1} GiB ({})",
                holder.pid,
                holder.process,
                holder.used_vram_gb,
                match (&holder.stado_job, &holder.unit, &holder.unit_error) {
                    (Some(job), _, _) => format!("Stado job {job}"),
                    (None, Some(unit), _) => format!("unit {unit}, not a Stado job"),
                    (None, None, Some(error)) => format!("unit unknown ({error}), not a Stado job"),
                    (None, None, None) => "not a Stado job".to_string(),
                }
            )
        })
        .collect();
    if unattributed > 0.0 {
        parts.push(format!("{unattributed:.1} GiB held by no listed process"));
    }
    Some(format!("held by {}", parts.join("; ")))
}
