//! Per-job ACTUAL GPU-memory measurement.
//!
//! Port of `stado/providers/local/helpers/gpu_probe.py`.
//!
//! The sizing heuristic must learn from the real number a job used, not
//! from what it declared. nvidia-smi's whole-GPU memory.used is unusable
//! for that on a co-scheduled agent (a neighbour job inflates it), so this
//! module attributes GPU memory per process: it sums the compute-app
//! used_memory over exactly the PIDs in the job's subprocess tree. The
//! agent samples this every advance_slot tick and records the peak onto
//! the Job before it moves running -> completed, where the sizing module
//! reads it back.

use std::collections::{HashMap, HashSet};

/// Pure parser for `ps -eo pid=,ppid=` output: (pid, ppid) rows. Lines that
/// do not split into exactly two integer fields are skipped (Python's
/// `len(parts) != 2` / `ValueError` guards).
pub fn parse_ps_pid_ppid(text: &str) -> Vec<(i32, i32)> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() != 2 {
                return None;
            }
            Some((parts[0].parse().ok()?, parts[1].parse().ok()?))
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

/// Python `_proc_tree_pids`. If ps is unreadable, only the root pid is
/// returned: that under-counts rather than mis-attributing a neighbour's
/// memory to this job.
pub async fn proc_tree_pids(root_pid: i32) -> HashSet<i32> {
    match tokio::process::Command::new("ps")
        .args(["-eo", "pid=,ppid="])
        .output()
        .await
    {
        Ok(out) if out.status.success() => tree_pids_from(
            &parse_ps_pid_ppid(&String::from_utf8_lossy(&out.stdout)),
            root_pid,
        ),
        _ => HashSet::from([root_pid]),
    }
}

/// Pure parser for `nvidia-smi --query-compute-apps=gpu_uuid,pid,used_memory
/// --format=csv,noheader,nounits`: (gpu_uuid, pid, used_mib) rows.
pub fn parse_compute_apps(text: &str) -> Vec<(String, i32, i64)> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(',').map(str::trim).collect();
            if parts.len() != 3 {
                return None;
            }
            Some((
                parts[0].to_string(),
                parts[1].parse().ok()?,
                parts[2].parse().ok()?,
            ))
        })
        .collect()
}

/// Pure attribution: sum the tree's used_memory within each gpu_uuid, then
/// return the MAX over GPUs (ceil GiB). 0 when no row matches the tree.
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
pub fn attributed_used_gb(rows: &[(String, i32, i64)], pids: &HashSet<i32>) -> i64 {
    let mut per_gpu_mib: HashMap<&str, i64> = HashMap::new();
    for (gpu_uuid, pid, mib) in rows {
        if pids.contains(pid) {
            *per_gpu_mib.entry(gpu_uuid.as_str()).or_insert(0) += mib;
        }
    }
    let Some(&peak_mib) = per_gpu_mib.values().max() else {
        return 0;
    };
    // Ceil to GiB (Python -(-peak_mib // 1024)); i64::div_ceil is unstable
    // on this toolchain.
    (peak_mib + 1023) / 1024
}

/// Python `smi_job_used_gb`: peak single-GPU memory (GiB, ceil) used by
/// root_pid's process tree. -1 if nvidia-smi is unreadable; 0 if the tree
/// currently holds no GPU memory (not yet allocated, or a CPU-only job).
pub async fn smi_job_used_gb(root_pid: i32) -> i64 {
    let out = match tokio::process::Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=gpu_uuid,pid,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .await
    {
        Ok(out) if out.status.success() => out,
        _ => return -1,
    };
    let pids = proc_tree_pids(root_pid).await;
    attributed_used_gb(
        &parse_compute_apps(&String::from_utf8_lossy(&out.stdout)),
        &pids,
    )
}
