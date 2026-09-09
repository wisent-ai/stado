//! The startup entry point: honour an explicit TMPDIR, detect a RAM-backed
//! `/tmp`, then rank every writable disk-backed candidate (SSD first, free
//! space second), publish the winner in TMPDIR and log what changed.

use super::*;

/// Detect a RAM-backed /tmp and redirect TMPDIR to disk. Returns the
/// chosen path, or None when nothing was changed.
/// Python `setup_agent_staging`.
pub async fn setup_agent_staging(log_fn: &mut dyn FnMut(&str)) -> Option<String> {
    let explicit = std::env::var("TMPDIR")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !explicit.is_empty() && !explicit.starts_with("/tmp") {
        log_fn(&format!(
            "staging: TMPDIR already set to {explicit}; keeping it"
        ));
        return Some(explicit);
    }
    if !tmp_is_tmpfs().await {
        return None;
    }
    let tmp_free = free_gb(Path::new("/tmp"));
    let user = agent_user();
    let mut best: Option<(String, f64, bool)> = None;
    for mnt in candidate_mounts() {
        let target: PathBuf = Path::new(&mnt).join("wisent-staging");
        if std::fs::create_dir_all(&target).is_err() {
            continue;
        }
        chown_if_root(&target, &user);
        if !writable_for_self(&target) {
            if euid() == 0 {
                try_repair_traversal(&target, log_fn);
                if !writable_for_self(&target) {
                    continue;
                }
            } else {
                log_fn(&format!(
                    "staging: candidate {} not writable by current user (parent perms?); skipping",
                    target.display()
                ));
                continue;
            }
        }
        let free = free_gb(&target);
        if free <= tmp_free {
            continue;
        }
        let rotational = is_rotational(&mnt);
        // Rank: prefer non-rotational (SSD/NVMe) over rotational (HDD);
        // break ties by free space. Shard staging is write-throughput
        // bound, so a smaller SSD beats a larger HDD.
        let better = match &best {
            None => true,
            Some((_, best_free, best_rotational)) => {
                let (rank, best_rank) = (!rotational, !*best_rotational);
                (rank && !best_rank) || (rank == best_rank && free > *best_free)
            }
        };
        if better {
            best = Some((target.to_string_lossy().into_owned(), free, rotational));
        }
    }
    let Some((target, free, rotational)) = best else {
        log_fn(
            "staging: /tmp is tmpfs but no larger writable disk-backed mount found; \
             staging stays on /tmp (RAM). Crashes possible.",
        );
        return None;
    };
    // Children inherit the env, so every job stages on disk for free.
    std::env::set_var("TMPDIR", &target);
    log_fn(&format!(
        "staging: redirected TMPDIR /tmp(tmpfs,{tmp_free:.0}G) -> {target} \
         ({}-backed, {free:.0}G free)",
        if rotational { "HDD" } else { "SSD" }
    ));
    Some(target)
}
