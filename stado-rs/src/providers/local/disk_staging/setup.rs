//! The startup entry point: honour an explicit TMPDIR, detect a RAM-backed
//! `/tmp`, then rank every writable disk-backed candidate (SSD first, free
//! space second), publish the winner in TMPDIR and log what changed.

use super::*;

/// Keep an explicit staging path or redirect a RAM-backed /tmp to disk.
/// Returns the retained or selected path, or None when no path was chosen.
/// Python `setup_agent_staging`.
pub async fn setup_agent_staging(log_fn: &mut dyn FnMut(&str)) -> Option<String> {
    // A TMPDIR that is not text is left alone and said: read as unset, it
    // was replaced by a staging directory of our choosing.
    let explicit = match crate::config_file::variable("TMPDIR") {
        Ok(Some(explicit)) => explicit,
        Ok(None) => String::new(),
        Err(cause) => {
            log_fn(&format!(
                "staging: {cause}; staging stays where TMPDIR points"
            ));
            return None;
        }
    };
    if !explicit.is_empty() && !Path::new(&explicit).starts_with("/tmp") {
        log_fn(&format!(
            "staging: TMPDIR already set to {explicit}; keeping it"
        ));
        return Some(explicit);
    }
    let tmp_fs = match tmp_ram_filesystem().await {
        Ok(Some(filesystem)) => filesystem,
        Ok(None) => return None,
        Err(cause) => {
            log_fn(&format!(
                "staging: {cause}; staging stays where TMPDIR points"
            ));
            return None;
        }
    };
    let tmp_free = match crate::providers::local::disk::gate::free_gb(Path::new("/tmp")) {
        Ok(free) => free,
        Err(cause) => {
            log_fn(&format!(
                "staging: /tmp is {tmp_fs} and its free space cannot be read ({cause}); no candidate \
                 can be compared with it, so staging stays on /tmp"
            ));
            return None;
        }
    };
    let user = match agent_user() {
        Ok(user) => user,
        Err(cause) => {
            log_fn(&format!(
                "staging: the agent user cannot be named ({cause}); staging stays on /tmp (RAM)"
            ));
            return None;
        }
    };
    let mut best: Option<(String, f64, Option<bool>)> = None;
    let mounts = match candidate_mounts(log_fn) {
        Ok(mounts) => mounts,
        Err(cause) => {
            log_fn(&format!(
                "staging: /tmp is {tmp_fs} and {cause}; no disk-backed mount can be chosen, so staging stays on /tmp (RAM)"
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
        if let Err(cause) = chown_if_root(&target, &user) {
            log_fn(&format!("staging: {cause}; skipping"));
            continue;
        }
        if let Err(first) = writable_for_self(&target) {
            if !nix::unistd::Uid::from_raw(euid()).is_root() {
                log_fn(&format!("staging: candidate {} not writable ({first}); skipping", target.display()));
                continue;
            }
            try_repair_traversal(&target, log_fn);
            if let Err(after) = writable_for_self(&target) {
                log_fn(&format!(
                    "staging: candidate {} not writable after the traversal repair ({after}); skipping",
                    target.display()
                ));
                continue;
            }
        }
        let free = match crate::providers::local::disk::gate::free_gb(&target) {
            Ok(free) => free,
            Err(cause) => {
                log_fn(&format!(
                    "staging: the free space of candidate {} cannot be read ({cause}); skipping",
                    target.display()
                ));
                continue;
            }
        };
        if free <= tmp_free {
            log_fn(&format!(
                "staging: candidate {} has {free:.1} GiB free, no more than /tmp's {tmp_free:.1} GiB; skipping",
                target.display()
            ));
            continue;
        }
        // Unknown media remains usable, but never pretends to be an HDD
        // and never outranks a device whose medium the kernel identified.
        let rotational = match is_rotational(&mnt) {
            Ok(rotational) => Some(rotational),
            Err(reason) => {
                log_fn(&format!(
                    "staging: cannot tell whether {mnt} is rotational ({reason}); ranked after identified disks"
                ));
                None
            }
        };
        // Known SSD, known HDD, then unknown; ties use measured free space.
        let better = match &best {
            None => true,
            Some((_, best_free, best_rotational)) => {
                let rank = rotational.map(std::cmp::Reverse);
                let best_rank = best_rotational.map(std::cmp::Reverse);
                rank > best_rank || (rank == best_rank && free > *best_free)
            }
        };
        if better {
            best = Some((target.to_string_lossy().into_owned(), free, rotational));
        }
    }
    let Some((target, free, rotational)) = best else {
        log_fn(
            &format!("staging: /tmp is {tmp_fs} but no larger writable disk-backed mount found; \
             staging stays on /tmp (RAM). Crashes possible."),
        );
        return None;
    };
    // Children inherit the env, so every job stages on disk for free.
    std::env::set_var("TMPDIR", &target);
    log_fn(&format!(
        "staging: redirected TMPDIR /tmp ({tmp_fs}, {tmp_free:.1} GiB free) -> {target} \
         ({}-backed, {free:.1} GiB free)",
        match rotational {
            Some(true) => "HDD",
            Some(false) => "SSD",
            None => "unknown-device",
        }
    ));
    Some(target)
}
