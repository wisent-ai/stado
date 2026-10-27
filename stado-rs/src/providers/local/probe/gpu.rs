//! Per-job ACTUAL GPU-memory measurement.
//!
//! The sizing heuristic must learn from the real number a job used, not
//! from what it declared. nvidia-smi's whole-GPU memory.used is unusable
//! for that on a co-scheduled agent (a neighbour job inflates it), so this
//! module attributes GPU memory per process: it sums the compute-app
//! used_memory over exactly the PIDs in the job's subprocess tree. The
//! agent samples this every advance_slot tick and records the peak onto
//! the Job before it moves running -> completed, where the sizing module
//! reads it back.
//!
//! The driver's rows are read by the accelerator holders' own parser
//! ([`parse_compute_apps_named`]), so a job's footprint and the holders table
//! read one row one way and refuse the same malformed row.

use std::collections::{HashMap, HashSet};

use crate::providers::local::accelerators::holders::parse_compute_apps_named;
use crate::providers::local::host_memory::constants::{GIB, MIB};

/// Pure parser for `ps -eo pid=,ppid=` output: (pid, ppid) rows, or the row
/// that is not two process ids.
pub fn parse_ps_pid_ppid(text: &str) -> Result<Vec<(i32, i32)>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let [pid, ppid] = parts.as_slice() else {
                return Err(format!("ps printed {line:?}, not a pid and its parent"));
            };
            let id = |value: &str| {
                value
                    .parse::<i32>()
                    .map_err(|error| format!("ps printed {line:?}: {value:?} is not a pid: {error}"))
            };
            Ok((id(pid)?, id(ppid)?))
        })
        .collect()
}

/// root_pid plus all descendants, from parsed `ps -eo pid=,ppid=` rows.
///
/// The job runs as `bash -c <cmd>` whose python child (and that child's
/// data-loader / nccl workers) are what actually allocate GPU memory, so
/// the whole subtree counts, not just root_pid.
pub fn tree_pids_from(rows: &[(i32, i32)], root_pid: i32) -> HashSet<i32> {
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for &(pid, ppid) in rows {
        children.entry(ppid).or_default().push(pid);
    }
    let mut seen = HashSet::new();
    let mut stack = vec![root_pid];
    while let Some(p) = stack.pop() {
        if !seen.insert(p) {
            continue;
        }
        if let Some(kids) = children.get(&p) {
            stack.extend(kids.iter().copied());
        }
    }
    seen
}

/// The job's process tree. If ps is unreadable, only the root pid is
/// returned: that under-counts rather than mis-attributing a neighbour's
/// memory to this job. The under-count is said on stderr with the cause, so a
/// job's GPU memory reading low is not mistaken for the job using little.
pub async fn proc_tree_pids(root_pid: i32) -> HashSet<i32> {
    let rows = match crate::wait::output_async(tokio::process::Command::new("ps").args(["-eo", "pid=,ppid="]))
        .await
    {
        Ok(out) if out.status.success() => parse_ps_pid_ppid(&String::from_utf8_lossy(&out.stdout)),
        Ok(out) => Err(format!("ps answered {} ({})", out.status, String::from_utf8_lossy(&out.stderr).trim())),
        Err(error) => Err(format!("ps could not be run ({error})")),
    };
    match rows {
        Ok(rows) => tree_pids_from(&rows, root_pid),
        Err(cause) => {
            eprintln!("gpu: {cause}; GPU memory is counted for pid {root_pid} alone, not its children");
            HashSet::from([root_pid])
        }
    }
}

/// Pure attribution: sum the tree's used memory within each card, then
/// return the largest card's sum in whole GiB, rounded up; `None` when no row
/// belongs to the tree.
///
/// gpu_mem_gb means "VRAM this job needs on the GPU it runs on": the fleet
/// is overwhelmingly single-GPU agents and a job loads onto one card.
/// nvidia-smi --query-compute-apps emits one row per (process, GPU), so a
/// process spread over N GPUs appears N times. Summing those rows yields
/// the CROSS-GPU total, not the per-card footprint: on the multi-GPU lab
/// box a sharded gpt-oss-20b summed to ~89 GiB while the same workload on
/// a single-GPU 80 GB agent measured ~50-74 GiB, and that inflated 89
/// (propagated by observed_vram_gb's max()) pinned every gpt-oss-20b job
/// above the entire single-GPU fleet.
///
/// So attribute PER GPU and take the largest footprint the job places on
/// any one card, invariant to how many GPUs the host has. A model that
/// genuinely cannot fit on one card is a separate "requires multi-GPU"
/// concept and must not be conflated into this scalar by accidental
/// summation.
pub fn attributed_used_gb(rows: &[(String, i32, String, i64)], pids: &HashSet<i32>) -> Option<i64> {
    let mut per_gpu_mib: HashMap<&str, i64> = HashMap::new();
    for (gpu_uuid, pid, _process, mib) in rows {
        if pids.contains(pid) {
            *per_gpu_mib.entry(gpu_uuid.as_str()).or_default() += mib;
        }
    }
    let peak_mib = *per_gpu_mib.values().max()?;
    // Whole GiB, rounded up: the negated floor of the negated quotient.
    let mib_per_gib = GIB / MIB;
    Some(-(-peak_mib).div_euclid(mib_per_gib))
}

/// Peak single-GPU memory (GiB, rounded up) root_pid's process tree holds
/// now; `None` when the tree holds no GPU memory (not yet allocated, a
/// CPU-only job, or a host with no NVIDIA driver at all), and why when the
/// driver is there and could not be read.
pub async fn smi_job_used_gb(root_pid: i32) -> Result<Option<i64>, String> {
    let out = match crate::wait::output_async(tokio::process::Command::new("nvidia-smi").args([
        "--query-compute-apps=gpu_uuid,pid,process_name,used_memory",
        "--format=csv,noheader,nounits",
    ]))
    .await
    {
        Ok(out) => out,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("nvidia-smi could not run: {error}")),
    };
    if !out.status.success() {
        return Err(format!(
            "nvidia-smi exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let rows = parse_compute_apps_named(&String::from_utf8_lossy(&out.stdout))?;
    let pids = proc_tree_pids(root_pid).await;
    Ok(attributed_used_gb(&rows, &pids))
}
