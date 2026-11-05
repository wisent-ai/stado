//! Docker's data on this host: images, the build cache, stopped containers
//! and volumes no container uses.
//!
//! The daemon keeps every image it pulled and every layer it built, under its
//! own root (`/var/lib/docker` by default), and never shrinks it. No cleaner
//! covered it, so a host serving models in containers filled its disk with
//! images it no longer ran and refused work at the disk-full rule. Under the
//! rule this cleaner asks the daemon itself to remove what nothing uses —
//! `docker system prune --all --volumes --force`, which keeps every running
//! container, the images those use and the volumes any container mounts — and
//! records what the daemon reported reclaimable before and what it says it
//! reclaimed. Removing files under the daemon's root directly would corrupt
//! its metadata, so the daemon is the only remover.
//!
//! A stopped container is removed too, so a service meant to be kept stopped
//! and started again must be a running one: inference deployments run with
//! `--restart unless-stopped`, and the sandbox runs `--rm`. While a job runs
//! nothing is taken, since a job may be pulling or building an image.
//!
//! Only a daemon whose root directory is on this host, on the volume the
//! rule judges (the home's, or the declared work root's), is pruned: pruning
//! any other frees nothing the rule measures. A daemon inside a virtual
//! machine (Docker Desktop) or on another disk is measured and kept, under
//! `docker_root_not_on_judged_volume`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::janitor::state::report::{CleanerReport, CleanupReport};

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "docker";

/// The client, named the way the hosts' services name it: Docker installs it
/// on the search path.
const DOCKER: &str = "docker";

/// What the daemon reports reclaimable now, summed over its kinds, in
/// bytes, from `docker system df --format '{{json .}}'` (one object per kind
/// with a human `Reclaimable` field such as `1.2GB (40%)`).
fn reclaimable() -> Result<(i64, i64), String> {
    let output = crate::wait::output(
        Command::new(DOCKER)
            .args(["system", "df", "--format", "{{json .}}"])
            .env("LC_ALL", "C"),
    )
    .map_err(|error| format!("docker could not be run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "docker system df refused: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut items = 0;
    let mut bytes = 0;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let row: Value = serde_json::from_str(line).map_err(|error| {
            format!("docker system df printed a row that is not JSON: {error}: {line}")
        })?;
        let reclaimable = row
            .get("Reclaimable")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("docker system df printed a row without Reclaimable: {line}"))?;
        let size = reclaimable.split_whitespace().next().unwrap_or(reclaimable);
        bytes += parse_size(size).ok_or_else(|| {
            format!("docker system df printed an unreadable size {size:?}: {line}")
        })?;
        let total = row
            .get("TotalCount")
            .and_then(Value::as_str)
            .and_then(|count| count.parse::<i64>().ok());
        let active = row
            .get("Active")
            .and_then(Value::as_str)
            .and_then(|count| count.parse::<i64>().ok());
        if let (Some(total), Some(active)) = (total, active) {
            items += (total - active).max(0);
        }
    }
    Ok((items, bytes))
}

/// Bytes in a size Docker prints (`0B`, `512kB`, `1.2GB`, `3.8TB`), decimal
/// units as Docker's own formatter writes them.
fn parse_size(text: &str) -> Option<i64> {
    let split = text.find(|character: char| character.is_ascii_alphabetic())?;
    let (number, unit) = text.split_at(split);
    let number: f64 = number.parse().ok()?;
    let scale: f64 = match unit {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "PB" => 1e15,
        _ => return None,
    };
    Some((number * scale) as i64)
}

/// The bytes the daemon says one prune reclaimed, from its last line
/// (`Total reclaimed space: 1.2GB`).
fn reclaimed(stdout: &str) -> Option<i64> {
    stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("Total reclaimed space:"))
        .and_then(|size| parse_size(size.trim()))
}

/// Whether the daemon's root directory is on the volume of `home` or of the
/// declared work root, or why not.
fn on_judged_volume(home: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let output =
        crate::wait::output(Command::new(DOCKER).args(["info", "--format", "{{.DockerRootDir}}"]))
            .map_err(|error| format!("docker could not be run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "docker info refused: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let root_device = std::fs::metadata(&root)
        .map_err(|error| {
            format!(
                "the daemon's root {} is not on this host ({error})",
                root.display()
            )
        })?
        .dev();
    let judged: Vec<PathBuf> = std::iter::once(home.to_path_buf())
        .chain(crate::providers::local::work_base::declared())
        .collect();
    for path in &judged {
        if std::fs::metadata(path).is_ok_and(|info| info.dev() == root_device) {
            return Ok(());
        }
    }
    Err(format!(
        "the daemon's root {} is on another volume than {}",
        root.display(),
        judged
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" and ")
    ))
}

/// Ask the daemon to remove what nothing uses, unless a job runs; a host
/// without Docker is recorded as such and touched nowhere.
pub fn scan_docker(home: &Path, enforcing: bool, report: &mut CleanupReport) {
    let mut record = CleanerReport::default();
    let mut skipped: Vec<&'static str> = Vec::new();
    match reclaimable() {
        Ok((items, bytes)) => {
            record.scanned_items = items;
            record.eligible_items = items;
            record.expected_bytes = bytes;
        }
        Err(error) => {
            // No client on the search path is a host without Docker, not a
            // failure of this pass.
            if error.starts_with("docker could not be run") {
                record.skipped.insert("docker_absent".to_string(), 1);
            } else {
                report.add_error(CLEANER, &super::JanitorError::os(&error));
            }
            report.docker = record;
            return;
        }
    }
    let placed = on_judged_volume(home);
    if let Err(cause) = &placed {
        skipped.push("docker_root_not_on_judged_volume");
        if !cause.starts_with("the daemon's root") {
            report.add_error(CLEANER, &super::JanitorError::os(cause));
        }
    }
    if placed.is_err() {
        // Measured and kept: the reason is counted above.
    } else if report.active_job_count.is_positive() && record.eligible_items > 0 {
        skipped.push("job_running");
    } else if enforcing && record.expected_bytes > 0 {
        let pruned = crate::wait::output(
            Command::new(DOCKER)
                .args(["system", "prune", "--all", "--volumes", "--force"])
                .env("LC_ALL", "C"),
        );
        match pruned {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                record.deleted_items = record.eligible_items;
                match reclaimed(&stdout) {
                    Some(bytes) => record.actual_free_delta_bytes = bytes,
                    None => report.add_error(
                        CLEANER,
                        &super::JanitorError::os(&format!(
                            "docker system prune printed no reclaimed total: {}",
                            stdout.trim()
                        )),
                    ),
                }
            }
            Ok(output) => {
                skipped.push("deletion_refused");
                report.add_error(
                    CLEANER,
                    &super::JanitorError::os(&format!(
                        "docker system prune refused: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    )),
                );
            }
            Err(error) => {
                skipped.push("deletion_refused");
                report.add_error(
                    CLEANER,
                    &super::JanitorError::os(&format!(
                        "docker system prune could not run: {error}"
                    )),
                );
            }
        }
    }
    let reasons: BTreeSet<&str> = skipped.iter().copied().collect();
    record.skipped = reasons
        .into_iter()
        .map(|reason| {
            let count = skipped.iter().filter(|seen| **seen == reason).count() as i64;
            (reason.to_string(), count)
        })
        .collect::<BTreeMap<String, i64>>();
    report.docker = record;
}
