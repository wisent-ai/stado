//! The launchd half of the post-swap reconcile: restart each domain-bound
//! job whose live image is not the file it declares, and verify the inode
//! afterwards.

use std::path::Path;

use super::recycle::defers_to_release_handshake;

/// `launchctl list` prints `PID\tStatus\tLabel` after one header row. A job
/// that is loaded but not running prints `-` for the PID and holds no image,
/// so it is skipped: it will pick up the new binary the next time launchd
/// starts it.
pub(super) async fn recycle_launchd(
    context: &str,
    paths: &[String],
    ours: u32,
    log_fn: &mut dyn FnMut(&str),
) -> Result<usize, String> {
    let registry = crate::cli::registry::read_registry()
        .await
        .map_err(|error| format!("{context}: cannot read registry unit ownership: {error}"))?;
    let hostname = crate::providers::vast::system_hostname();
    let target = registry
        .lookup_self(&hostname)
        .map_err(|error| format!("{context}: cannot identify this host: {error}"))?
        .ok_or_else(|| format!("{context}: no registry target names this machine ({hostname})"))?;
    let runner = crate::deploy::production_runner();
    let units = crate::deploy::service::loaded_image_units(target, &runner)
        .await
        .map_err(|error| {
            format!("{context}: cannot enumerate domain-bound launchd units: {error}")
        })?;
    let pids: Vec<u32> = units
        .iter()
        .filter_map(|unit| unit.pid.parse().ok())
        .filter(|pid| *pid != ours)
        .collect();
    let running_images = crate::deploy::service::running_images(&pids)
        .map_err(|error| format!("{context}: cannot read running image identities: {error}"))?;
    let installed_images: Vec<(String, crate::deploy::service::ImageIdentity)> = paths
        .iter()
        .map(|path| {
            crate::deploy::service::installed_image(Path::new(path))
                .map(|(image, _)| (path.clone(), image))
                .map_err(|error| format!("{context}: cannot identify installed {path}: {error}"))
        })
        .collect::<Result<_, _>>()?;
    let mut restarted = 0usize;
    for unit in units {
        let Ok(pid) = unit.pid.parse::<u32>() else {
            continue;
        };
        if pid == ours {
            continue;
        }
        let running = running_images.get(&pid);
        let declared_program = unit.program.split_whitespace().next();
        let directly_declared = paths
            .iter()
            .any(|path| declared_program == Some(path.as_str()));
        if directly_declared && running.is_none() {
            return Err(format!(
                "{context}: the kernel image for {} pid {pid} is unreadable",
                unit.label
            ));
        }
        let selected = running.and_then(|running| {
            installed_images.iter().find(|(path, installed)| {
                (declared_program == Some(path.as_str())
                    || running.path.trim_end_matches(" (deleted)") == path)
                    && !running.is_same_file(installed)
            })
        });
        let Some((program, installed)) = selected else {
            continue;
        };
        let argv: Vec<&str> = unit.running_program.split_whitespace().collect();
        if defers_to_release_handshake(&argv) {
            log_fn(&format!(
                "{context}: {} is running the replaced {program} and recycles itself through \
                 the installed-release handshake, so it was left to finish its slot",
                unit.label
            ));
            continue;
        }
        if unit.loaded_domains.len() != 1 {
            return Err(format!(
                "{context}: {} pid {pid} executes replaced {program}, but launchd reports {} \
                 loaded domains; refusing to guess which job owns the pid",
                unit.label,
                unit.loaded_domains.len()
            ));
        }
        let service = crate::deploy::service::restart_local_unit(
            target,
            &unit.label,
            &unit.path,
            Some(&unit.loaded_domains[0]),
        )
        .await
        .map_err(|error| {
            format!(
                "{context}: {} was executing the replaced {program} and could not be restarted \
                 through its observed owner {}/{} and declared unit {}: {error}",
                unit.label, unit.loaded_domains[0], unit.label, unit.path
            )
        })?;
        let after = crate::deploy::service::loaded_image_units(target, &runner)
            .await
            .map_err(|error| format!("{context}: cannot re-read {service}: {error}"))?;
        let current_pid = after
            .iter()
            .find(|current| {
                current.label == unit.label && current.loaded_domains == unit.loaded_domains
            })
            .and_then(|current| current.pid.parse::<u32>().ok())
            .ok_or_else(|| format!("{context}: {service} restarted without a readable pid"))?;
        let images = crate::deploy::service::running_images(&[current_pid])
            .map_err(|error| format!("{context}: cannot verify {service}'s new image: {error}"))?;
        if !images
            .get(&current_pid)
            .is_some_and(|running| running.is_same_file(installed))
        {
            return Err(format!(
                "{context}: {service} restarted but pid {current_pid} does not execute the installed inode at {program}"
            ));
        }
        log_fn(&format!(
            "{context}: reconciled {service}; pid {pid} was running a different image, \
             and pid {current_pid} now executes the installed inode at {program}"
        ));
        restarted += 1;
    }
    Ok(restarted)
}
