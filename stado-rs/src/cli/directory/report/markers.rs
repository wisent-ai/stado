//! The `~/.stado/forwards/<service>.local` files: the address a host dials for
//! a service placed elsewhere, the writer that produces one, and the sweep
//! that names every marker no declaration accounts for.

use serde_json::Value;

use crate::cli::registry;
use crate::cli::CmdError;

/// What a host dials for a service the directory places somewhere else: the
/// bind of its own resolver adapter, from the target's declared
/// `service_resolver`.
///
/// The marker is the address consumers on THIS host use, and for a service
/// served elsewhere that address is never the serving host's own loopback
/// port. Publishing skipped those services entirely, so the file either did
/// not exist or still held whatever wrote it last: on `lukasz-macbook`
/// `brama.local` named `127.0.0.1:8080`, which on that machine belongs to an
/// unrelated service, and `weles-admission.local` named `8788` while the
/// documented answer for a non-serving host is its adapter at `17614`. Every
/// consumer reading those files dialled the wrong thing for as long as they
/// existed.
///
/// One adapter is an address; several are a question this function may not
/// answer, because adapters are per consumer and the marker's name carries no
/// consumer. `Err` names them so the caller reports the ambiguity instead of
/// electing one consumer's socket for everybody.
pub(super) fn adapter_url(target_entry: &Value, service: &str) -> Result<Option<String>, String> {
    let Some(declared) = target_entry.get("service_resolver") else {
        return Ok(None);
    };
    let config: crate::service_resolution::ResolverConfig =
        serde_json::from_value(declared.clone())
            .map_err(|error| format!("target service_resolver is invalid: {error}"))?;
    let mut matches = config
        .adapters
        .iter()
        .filter(|adapter| adapter.service == service)
        .peekable();
    let Some(first) = matches.next() else {
        return Ok(None);
    };
    if matches.peek().is_some() {
        let mut consumers: Vec<&str> = vec![first.consumer.as_str()];
        consumers.extend(matches.map(|adapter| adapter.consumer.as_str()));
        return Err(format!(
            "this host's resolver declares {} {service} adapters, one per consumer ({}), and a \
             marker names no consumer; read the address from the consumer's own adapter",
            consumers.len(),
            consumers.join(", ")
        ));
    }
    Ok(Some(format!("http://{}", first.bind)))
}

/// A sweep that could not remove everything it named, reported after the
/// listing rather than instead of it.
///
/// The fossils are printed either way -- they are the finding, and they are
/// still true when an unlink fails -- but the command must not exit zero, or a
/// caller that runs this to convergence will believe the directory is clean.
pub(super) fn prune_outcome(failed: usize) -> Result<(), CmdError> {
    if failed == usize::default() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{failed} forward marker(s) could not be removed; each is reported above with the \
         filesystem's own words"
    )))
}

/// One `~/.stado/forwards/<name>.local` no declaration accounts for.
pub(super) struct FossilMarker {
    /// The service the marker's name claims, which is exactly the name a
    /// consumer on this host resolves through it.
    pub(super) service: String,
    pub(super) marker: std::path::PathBuf,
    /// The address the file hands out. Printed rather than summarised: three
    /// of these on one host name one endpoint, and two name one relationship
    /// on two wrong ports, neither of which is visible from the filenames.
    pub(super) value: String,
    /// `None` means the filesystem would not say, never "new". An unknown age
    /// rendered as zero would sort a fossil to the safe end of the list.
    pub(super) age_seconds: Option<i64>,
}

impl FossilMarker {
    pub(super) fn age(&self) -> String {
        self.age_seconds.map_or_else(
            || "unknown".to_string(),
            |seconds| registry::human_age(chrono::TimeDelta::seconds(seconds)),
        )
    }
}

/// What is on disk, against what the directory declares.
pub(super) struct MarkerSweep {
    /// Every marker present, declared or not. The denominator: "8 fossils" is
    /// a different sentence from "8 of 11".
    pub(super) present: usize,
    /// Undeclared markers, oldest first, because the oldest is the one most
    /// likely to be held by something nobody remembers writing.
    pub(super) fossil: Vec<FossilMarker>,
}

/// Compare the forwards directory against the declared set.
///
/// Read-only. Removal is the caller's decision under an explicit flag, and
/// keeping the two apart is what makes the report safe to run on every publish.
///
/// Only `<name>.local` regular files are considered. A staging file left by an
/// interrupted write is named `<name>.local.staging` and is not a marker; a
/// symlink is not something `write_forward_marker` can have produced, and
/// following one would report -- and under `--prune` delete -- an unrelated
/// path.
pub(super) fn sweep_markers(
    forwards: &std::path::Path,
    declared: &std::collections::BTreeSet<&str>,
) -> Result<MarkerSweep, CmdError> {
    let mut sweep = MarkerSweep {
        present: usize::default(),
        fossil: Vec::new(),
    };
    for entry in std::fs::read_dir(forwards)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(service) = name.strip_suffix(".local") else {
            continue;
        };
        if service.is_empty() || service.starts_with('.') {
            continue;
        }
        let marker = entry.path();
        let metadata = marker.symlink_metadata()?;
        if !metadata.is_file() {
            continue;
        }
        sweep.present += 1;
        if declared.contains(service) {
            continue;
        }
        // An unreadable marker is still a fossil: the consumer reading it is
        // running as the owner and may well succeed where this did not, so
        // dropping the row would hide an address that still resolves.
        let value = std::fs::read_to_string(&marker).map_or_else(
            |error| format!("(unreadable: {error})"),
            |content| {
                content
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            },
        );
        sweep.fossil.push(FossilMarker {
            service: service.to_string(),
            marker,
            value,
            age_seconds: marker_age(&metadata),
        });
    }
    // Oldest first: the marker nobody has rewritten in weeks is the one most
    // likely to be a fossil worth removing.
    sweep
        .fossil
        .sort_by_key(|marker| std::cmp::Reverse(marker.age_seconds));
    Ok(sweep)
}

/// How long ago the marker was written, in seconds.
///
/// A modification time in the future is clock skew, not a marker written
/// tomorrow; `duration_since` refuses it and the row reads `unknown`, which is
/// the honest answer and keeps it out of the oldest-first head of the list.
fn marker_age(metadata: &std::fs::Metadata) -> Option<i64> {
    let written = metadata.modified().ok()?;
    let age = std::time::SystemTime::now().duration_since(written).ok()?;
    i64::try_from(age.as_secs()).ok()
}

pub(super) fn write_forward_marker(marker: &std::path::Path, url: &str) -> Result<(), CmdError> {
    use std::os::unix::fs::PermissionsExt;

    let owner_only = u32::from_str_radix("600", "8".parse().unwrap_or_default())
        .map_err(|error| CmdError::click(error.to_string()))?;
    let staging = marker.with_extension("local.staging");
    std::fs::write(&staging, format!("{url}\n"))?;
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(owner_only))?;
    std::fs::rename(&staging, marker)?;
    Ok(())
}
