//! A Stado release that failed on this host, and the way back from it.
//!
//! An installed Stado whose stop proves its install failed (its rules refuse
//! the configuration the release before it served, or the same build stops
//! again here: `runtime::stops::failed_install`) cannot serve on this host,
//! and every adapter of the host goes down with it. So the failing build
//! records itself as failed beside its retained archive, with its source
//! revision, and reinstalls the release that ran before it (byte for byte
//! `~/.stado/bin/stado.previous`) as `release local restore --previous`
//! does. That build is never installed here again until the record is
//! removed, so two failing builds do not restore each other in turn; each
//! health pass publishes the failed builds in the beacon, and a delivery to
//! any host refuses one another host published. `stado release local list`
//! reads every retained release with what it is to this host.

use std::path::{Path, PathBuf};

use clap::Args;
use serde_json::{json, Value};

use crate::cli::CmdError;

/// The record beside a retained release that says it failed on this host.
const FAILED_RECORD: &str = "failed-on-this-host.json";

/// Where the failure record of `version` lives on this host.
fn record(version: &str) -> Result<PathBuf, CmdError> {
    let platform = crate::self_update::platform_triple_short()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    Ok(crate::config_file::expand_tilde("~")
        .join(".stado")
        .join("releases")
        .join("stado")
        .join(version)
        .join(platform)
        .join(FAILED_RECORD))
}

/// Refuse the build `program` (an incoming Stado of `version`) when this
/// host recorded that build as failed, naming the record and why. Records
/// name the failed build's revision: every revision shares the version, so
/// a refusal by version refused every later repair too; a record written
/// before records named one refuses the version. A `delivery` is also
/// refused a build any host published as failed in its beacon
/// (`failed_releases`); a restore is not, because it runs while this host's
/// services may be down and reinstalls a release that ran here before.
pub(super) async fn refuse_failed(version: &str, program: &Path, delivery: bool) -> Result<(), CmdError> {
    let revision = build_revision(program)?;
    let same_build = |record: &Value| {
        record.get("revision").and_then(Value::as_str).is_none_or(|failed| failed == revision)
    };
    let path = record(version)?;
    match std::fs::read_to_string(&path) {
        Ok(text) if serde_json::from_str::<Value>(&text).map_or(true, |record| same_build(&record)) => {
            return Err(CmdError::refused(format!(
                "Stado {version} (rev {revision}) failed on this host and is not installed here again: {}; the record is {} \
                 — remove it once the cause is repaired",
                text.trim(),
                path.display()
            )))
        }
        Ok(_another_build) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CmdError::click(format!(
                "cannot read whether Stado {version} failed on this host from {}: {error}",
                path.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind())))
        }
    }
    if !delivery {
        return Ok(());
    }
    let unread = |error: CmdError| {
        CmdError::click(format!(
            "cannot read whether another host published Stado {version} as failed, so it is not \
             delivered here: the fleet's beacons could not be read: {error}"
        ))
    };
    let store = crate::cli::host::beacon_store().await.map_err(unread)?;
    let beacons = crate::cli::registry::beacon_bodies(&store)
        .await
        .map_err(unread)?;
    for (host, beacon) in beacons {
        let Some(Value::Array(failed)) = beacon.get("failed_releases") else {
            continue;
        };
        if let Some(record) = failed.iter().find(|failed| {
            failed.get("version").and_then(Value::as_str) == Some(version)
                && failed.get("record").is_none_or(same_build)
        }) {
            return Err(CmdError::refused(format!(
                "Stado {version} failed on {host} and is not delivered to another host: {record}; \
                 {host} drops it from its beacon once its record is removed there"
            )));
        }
    }
    Ok(())
}

/// The source revision `program` says it was built from (`stado <version>
/// (rev <revision>)`), or the refusal naming what it said instead.
fn build_revision(program: &Path) -> Result<String, CmdError> {
    let output = crate::wait::output(std::process::Command::new(program).arg("--version"))
        .map_err(|error| CmdError::click(format!("{} --version could not run: {error}", program.display())))?;
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    said.split_once("(rev ")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(revision, _)| revision.to_owned())
        .ok_or_else(|| CmdError::refused(format!("{} --version names no source revision: {}", program.display(), said.trim())))
}

/// The releases this host recorded as failed, as its beacon publishes them
/// for the fleet: each with its platform and its record.
pub(crate) fn failed_here() -> Result<Value, CmdError> {
    let (platform, _, releases) = retained()?;
    Ok(Value::Array(
        releases
            .into_iter()
            .filter_map(|release| {
                release.failed.map(|record| {
                    json!({ "version": release.version, "platform": platform, "record": record })
                })
            })
            .collect(),
    ))
}

/// Record the running release as failed on this host for `cause` (why its
/// stop proved the install failed, as `runtime::stops::failed_install`
/// says it), then reinstall the release that ran before it, and say so on
/// every alert channel the operator enabled, so a host that rolled back is
/// not found later by somebody reading its service manager. The answer says
/// what was done and what each alert channel answered.
pub(crate) async fn roll_back_failed_release(cause: &str) -> Result<String, CmdError> {
    let version = env!("CARGO_PKG_VERSION");
    let path = record(version)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            CmdError::click(format!("cannot prepare {}: {error}", parent.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    let written = serde_json::json!({
        "version": version,
        "pid": std::process::id(),
        "at": chrono::Utc::now().to_rfc3339(),
        "cause": cause,
        "revision": crate::binary::build_identity::SOURCE_REVISION,
    });
    std::fs::write(&path, format!("{written}\n")).map_err(|error| {
        CmdError::click(format!(
            "cannot record Stado {version} as failed on this host at {}: {error}",
            path.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let restored = super::restore::restore_previous().await?;
    let said = format!(
        "Stado {version} failed on this host ({cause}); recorded as failed at {} and \
         reinstalled Stado {restored}, the release that ran before it",
        path.display()
    );
    let alerted = crate::monitor::alerts::send_alert(
        crate::config::alerts_topic(),
        &said,
        "Stado rolled back a release that failed on this host",
    )
    .await;
    let channels: Vec<String> = alerted
        .iter()
        .map(|delivery| match delivery.delivered {
            true => format!("{} took it ({})", delivery.channel, delivery.detail),
            false => format!("{} refused it: {}", delivery.channel, delivery.detail),
        })
        .collect();
    Ok(match channels.is_empty() {
        true => format!("{said}; no alert channel is enabled, so nobody was told"),
        false => format!("{said}; alert: {}", channels.join("; ")),
    })
}

#[derive(Args)]
pub struct ReleaseListLocalArgs {
    /// Print the releases as one JSON document.
    #[arg(long)]
    json: bool,
}

fn unreadable(path: &std::path::Path, error: std::io::Error) -> CmdError {
    CmdError::click(format!(
        "retained Stado releases: {} cannot be read: {error}",
        path.display()
    ))
    .stating(crate::cli::entry::error::io_failure_code(error.kind()))
}

/// The file's bytes, or nothing when it is absent; any other failure names it.
fn read_if_present(path: &std::path::Path) -> Result<Option<Vec<u8>>, CmdError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unreadable(path, error)),
    }
}

/// One retained release and what it is to this host.
struct Retained {
    version: String,
    installed: bool,
    rollback_target: bool,
    failed: Option<Value>,
}

/// Every Stado release this host retains, with what it is to the host: the
/// platform, the directory they are retained under, and the releases.
fn retained() -> Result<(&'static str, PathBuf, Vec<Retained>), CmdError> {
    let platform = crate::self_update::platform_triple_short()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    let stado = crate::config_file::expand_tilde("~").join(".stado");
    let retained = stado.join("releases").join("stado");
    let installed = read_if_present(&stado.join("bin").join("stado.release-version"))?
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string());
    let previous = read_if_present(&stado.join("bin").join("stado.previous"))?;
    let entries = match std::fs::read_dir(&retained) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| unreadable(&retained, error))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(unreadable(&retained, error)),
    };
    let mut releases: Vec<Retained> = Vec::new();
    for entry in entries {
        let version = entry.file_name().to_string_lossy().into_owned();
        let directory = entry.path().join(platform);
        if !directory.is_dir() {
            continue;
        }
        let staged = read_if_present(&directory.join("stado"))?;
        let record = directory.join(FAILED_RECORD);
        let failed = match read_if_present(&record)? {
            Some(bytes) => Some(serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                CmdError::click(format!(
                    "retained Stado releases: {} is not a failure record: {error}",
                    record.display()
                ))
            })?),
            None => None,
        };
        releases.push(Retained {
            installed: installed.as_deref() == Some(version.as_str()),
            rollback_target: previous.is_some() && staged == previous,
            failed,
            version,
        });
    }
    releases.sort_by(|left, right| left.version.cmp(&right.version));
    Ok((platform, retained, releases))
}

/// Every Stado release this host retains, with what it is to the host.
pub(in crate::cli::release_cmd) async fn list_local(
    args: &ReleaseListLocalArgs,
) -> Result<(), CmdError> {
    let (platform, retained, releases) = retained()?;
    if args.json {
        let listed: Vec<Value> = releases
            .iter()
            .map(|release| {
                json!({ "version": release.version, "installed": release.installed,
                    "rollbackTarget": release.rollback_target, "failed": release.failed })
            })
            .collect();
        println!("{}", json!({ "platform": platform, "releases": listed }));
        return Ok(());
    }
    if releases.is_empty() {
        println!(
            "this host retains no Stado release under {}",
            retained.display()
        );
    }
    for release in &releases {
        let mut roles = Vec::new();
        if release.installed {
            roles.push("installed".to_string());
        }
        if release.rollback_target {
            roles.push("rollback target".to_string());
        }
        if let Some(record) = &release.failed {
            roles.push(format!("failed on this host: {record}"));
        }
        println!("{}  {}", release.version, roles.join("; "));
    }
    Ok(())
}
