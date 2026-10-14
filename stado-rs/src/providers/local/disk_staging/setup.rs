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
    match tmp_is_tmpfs().await {
        Ok(true) => {}
        Ok(false) => return None,
        Err(cause) => {
            log_fn(&format!(
                "staging: {cause}; staging stays where TMPDIR points"
            ));
            return None;
        }
    }
    let tmp_free = match crate::providers::local::disk::gate::free_gb(Path::new("/tmp")) {
        Ok(free) => free,
        Err(cause) => {
            log_fn(&format!(
                "staging: /tmp is tmpfs and {cause}; no candidate can be compared with it, \
                 so staging stays on /tmp"
            ));
            return None;
        }
    };
    let user = agent_user();
    let mut best: Option<(String, f64, bool)> = None;
    let mounts = match candidate_mounts() {
        Ok(mounts) => mounts,
        Err(cause) => {
            log_fn(&format!(
                "staging: /tmp is tmpfs and {cause}; no disk-backed mount can be chosen, so staging stays on /tmp (RAM)"
            ));
            return None;
        }
    };
    for mnt in mounts {
        let target: PathBuf = Path::new(&mnt).join("wisent-staging");
        if let Err(error) = std::fs::create_dir_all(&target) {
            log_fn(&format!(
                "staging: candidate {} cannot be created ({error}); skipping",
                target.display()
            ));
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
        let free = match crate::providers::local::disk::gate::free_gb(&target) {
            Ok(free) => free,
            Err(cause) => {
                log_fn(&format!("staging: candidate {cause}; skipping"));
                continue;
            }
        };
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
