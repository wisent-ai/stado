//! Target-scoped delivery of one local file or directory into a managed area
//! of the registry-approved account's home.
//!
//! The destination is home-relative by construction: callers know a Stado
//! target and a managed relative path, never its SSH account or home. Before
//! any byte is transferred, the host checks every existing destination
//! component with `lstat` semantics (`test -L`), refuses foreign ownership or
//! the wrong file kind, and reserves a same-parent staging path. `rsync -a`
//! carries uncommitted working-tree bytes, application bundles, modes and
//! symlinks through the target's selected Stado SSH route. Only after rsync
//! succeeds does one guarded rename replace the destination.

use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path};
use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

use super::{host_channel, shlex_quote, ssh_key, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

pub const DELIVERED_STATUS: &str = "delivered";
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MARKER: &str = "STADO_DELIVER";
const MANAGED_RUNS_ROOT: &str = ".stado/work/runs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    File,
    Directory,
}

impl SourceKind {
    fn word(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

#[derive(Debug)]
struct DeliveryPlan {
    source: String,
    destination: String,
    kind: SourceKind,
    root_mode: u32,
    file_list: Option<String>,
}

fn safe_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && component
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Validate the destination without consulting a host. Delivery is confined
/// to one run UUID below Stado's managed work root; the final component names
/// the source tree or application bundle within that run.
fn destination_components(destination: &str) -> Result<Vec<&str>, DeployError> {
    if destination.starts_with('/') || destination.starts_with('~') || destination.contains('\0') {
        return Err(DeployError(format!(
            "destination {destination:?} is outside the managed area; use a path relative to the approved account's home under {MANAGED_RUNS_ROOT}/<RUN-UUID>/"
        )));
    }
    let path = Path::new(destination);
    let components = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => value
                .to_str()
                .ok_or_else(|| DeployError("destination must be UTF-8".to_string())),
            _ => Err(DeployError(format!(
                "destination {destination:?} must contain only ordinary path components and no '..'"
            ))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty() || components.iter().any(|component| !safe_component(component)) {
        return Err(DeployError(format!(
            "destination {destination:?} must contain only path components made of letters, digits, '.', '_' or '-' and no '..'"
        )));
    }
    let runs: Vec<&str> = MANAGED_RUNS_ROOT.split('/').collect();
    if !components.starts_with(&runs) || components.len() < runs.len() + 2 {
        return Err(DeployError(format!(
            "destination {destination:?} is outside the managed area; use a path relative to the approved account's home under {MANAGED_RUNS_ROOT}/<RUN-UUID>/"
        )));
    }
    let run = components[runs.len()];
    let canonical = Uuid::parse_str(run)
        .ok()
        .map(|value| value.hyphenated().to_string())
        .is_some_and(|value| value == run);
    if !canonical {
        return Err(DeployError(format!(
            "destination {destination:?} does not name a canonical lowercase UUID below {MANAGED_RUNS_ROOT}"
        )));
    }
    Ok(components)
}

fn validate_file_list(raw: Option<&str>, kind: SourceKind) -> Result<Option<String>, DeployError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if kind != SourceKind::Directory {
        return Err(DeployError(
            "--files-from is valid only when SOURCE is a directory".to_string(),
        ));
    }
    if raw.is_empty() {
        return Err(DeployError(
            "--files-from is empty; refusing an accidental empty-tree replacement".to_string(),
        ));
    }
    if !raw.ends_with('\0') {
        return Err(DeployError(
            "--files-from must be NUL-delimited and end with NUL".to_string(),
        ));
    }
    for entry in raw[..raw.len() - 1].split('\0') {
        let path = Path::new(entry);
        if entry.is_empty()
            || path.is_absolute()
            || path.components().any(|component| {
                !matches!(component, Component::Normal(_))
                    || matches!(component, Component::ParentDir)
            })
        {
            return Err(DeployError(format!(
                "--files-from entry {entry:?} is not a relative path below SOURCE"
            )));
        }
    }
    Ok(Some(raw.to_string()))
}

fn plan(source: &str, destination: &str, file_list: Option<&str>) -> Result<DeliveryPlan, DeployError> {
    let components = destination_components(destination)?;
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| DeployError(format!("cannot read delivery source {source:?}: {error}")))?;
    if metadata.file_type().is_symlink() {
        return Err(DeployError(
            "delivery source must be a regular file or directory, not a symlink".to_string(),
        ));
    }
    let kind = if metadata.is_file() {
        SourceKind::File
    } else if metadata.is_dir() {
        SourceKind::Directory
    } else {
        return Err(DeployError(
            "delivery source must be a regular file or directory".to_string(),
        ));
    };
    let destination = components.join("/");
    Ok(DeliveryPlan {
        source: source.to_string(),
        destination,
        kind,
        root_mode: metadata.permissions().mode() & 0o7777,
        file_list: validate_file_list(file_list, kind)?,
    })
}

fn guard_lines(home: &str, components: &[&str], include_destination: bool) -> String {
    let mut lines = String::new();
    let count = if include_destination {
        components.len()
    } else {
        components.len().saturating_sub(1)
    };
    for index in 0..count {
        let path = format!("{home}/{}", components[..=index].join("/"));
        let quoted = shlex_quote(&path);
        lines.push_str(&format!(
            "if [ -L {quoted} ]; then report refused {}; exit 0; fi\n",
            shlex_quote(&format!("destination traverses a symlink at {path}"))
        ));
        if index + 1 < components.len() {
            lines.push_str(&format!(
                "if [ -e {quoted} ]; then [ -d {quoted} ] || {{ report refused {}; exit 0; }}; [ -O {quoted} ] || {{ report refused {}; exit 0; }}; else /bin/mkdir {quoted}; /bin/chmod 700 {quoted}; fi\n",
                shlex_quote(&format!("destination parent is not a directory: {path}")),
                shlex_quote(&format!("destination parent is not owned by the approved account: {path}")),
            ));
        }
    }
    lines
}

fn parse_marker(target: &ComputeTarget, output: &super::CommandOutput) -> Result<(String, String), DeployError> {
    let fields = output
        .stdout
        .lines()
        .find_map(|line| {
            let fields = host_channel::marker_fields(line);
            (fields.first().copied() == Some(MARKER)).then_some(fields)
        })
        .ok_or_else(|| {
            DeployError(format!(
                "{}: the host answered without a delivery report: {}",
                target.name,
                host_channel::last_error_line(output, "no marker in output")
            ))
        })?;
    let status = fields.get(1).copied().unwrap_or_default().to_string();
    let detail = fields.get(2).copied().unwrap_or_default().to_string();
    Ok((status, detail))
}

async fn preflight(
    target: &ComputeTarget,
    home: &str,
    plan: &DeliveryPlan,
    runner: &Runner,
) -> Result<(String, String), DeployError> {
    let components = plan.destination.split('/').collect::<Vec<_>>();
    let destination = format!("{home}/{}", plan.destination);
    let parent = Path::new(&destination)
        .parent()
        .and_then(Path::to_str)
        .ok_or_else(|| DeployError("destination has no parent directory".to_string()))?;
    let name = components.last().copied().unwrap_or_default();
    let stage = format!("{parent}/.{name}.stado-delivery");
    let backup = format!("{parent}/.{name}.stado-previous");
    let stage_q = shlex_quote(&stage);
    let expected_test = match plan.kind {
        SourceKind::File => "-f",
        SourceKind::Directory => "-d",
    };
    let wrong_kind = format!(
        "destination exists but is not a {}: {destination}",
        plan.kind.word()
    );
    let script = format!(
        "set -eu\numask 077\nreport() {{ printf '{MARKER}\\t%s\\t%s\\n' \"$1\" \"$2\"; }}\n{}\n\
         if [ -L {destination_q} ]; then report refused {destination_symlink}; exit 0; fi\n\
         if [ -e {destination_q} ]; then [ {expected_test} {destination_q} ] || {{ report refused {wrong_kind}; exit 0; }}; [ -O {destination_q} ] || {{ report refused {foreign_destination}; exit 0; }}; fi\n\
         if [ -L {stage_q} ]; then report refused {stage_symlink}; exit 0; fi\n\
         if [ -e {stage_q} ]; then [ -O {stage_q} ] || {{ report refused {foreign_stage}; exit 0; }}; /bin/rm -rf -- {stage_q}; fi\n\
         if [ -e {backup_q} ] || [ -L {backup_q} ]; then report refused {backup_exists}; exit 0; fi\n\
         {}\nreport ready {stage_q}\n",
        guard_lines(home, &components, false),
        if plan.kind == SourceKind::Directory {
            format!("/bin/mkdir {stage_q}; /bin/chmod 700 {stage_q}")
        } else {
            String::new()
        },
        destination_q = shlex_quote(&destination),
        destination_symlink = shlex_quote(&format!("destination traverses a symlink at {destination}")),
        wrong_kind = shlex_quote(&wrong_kind),
        foreign_destination = shlex_quote(&format!("destination is not owned by the approved account: {destination}")),
        stage_q = stage_q,
        stage_symlink = shlex_quote(&format!("delivery staging path is a symlink: {stage}")),
        foreign_stage = shlex_quote(&format!("delivery staging path is not owned by the approved account: {stage}")),
        backup_q = shlex_quote(&backup),
        backup_exists = shlex_quote(&format!("previous-delivery recovery path already exists: {backup}")),
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let (status, detail) = parse_marker(target, &output)?;
    if status != "ready" {
        return Err(DeployError(format!(
            "{}: delivery refused before transfer: {detail}",
            target.name
        )));
    }
    Ok((destination, stage))
}


async fn transfer(
    target: &ComputeTarget,
    stage: &str,
    plan: &DeliveryPlan,
    runner: &Runner,
) -> Result<(), DeployError> {
    let mut argv = vec!["rsync".to_string(), "-a".to_string()];
    let stdin = if let Some(file_list) = &plan.file_list {
        // rsync deliberately drops `-r` from `-a` when `--files-from` is
        // present; spell it back in so selected nested source trees remain
        // recursive.
        argv.extend([
            "-r".to_string(),
            "--delete".to_string(),
            "--from0".to_string(),
            "--files-from=-".to_string(),
        ]);
        Some(file_list.clone())
    } else {
        if plan.kind == SourceKind::Directory {
            argv.push("--delete".to_string());
        }
        None
    };
    let source = if plan.kind == SourceKind::Directory {
        format!("{}/", plan.source.trim_end_matches('/'))
    } else {
        plan.source.clone()
    };
    let stage_argument = if plan.kind == SourceKind::Directory {
        format!("{stage}/")
    } else {
        stage.to_string()
    };
    if host_channel::target_is_this_host(target) {
        argv.extend(["--".to_string(), source, stage_argument]);
    } else {
        let connection = host_channel::select_ssh_connection(target, runner).await?;
        let key = ssh_key::materialize(&target.name).await?;
        let mut ssh = host_channel::ssh_options(connection.destination);
        ssh.pop();
        let ssh = ssh_key::add_identity(ssh, &key)?;
        let remote_shell = ssh
            .iter()
            .map(|word| shlex_quote(word))
            .collect::<Vec<_>>()
            .join(" ");
        argv.extend([
            "-e".to_string(),
            remote_shell,
            "--".to_string(),
            source,
            format!("{}:{stage_argument}", connection.destination),
        ]);
        let output = runner(CommandSpec {
            argv,
            stdin,
            timeout: Some(TRANSFER_TIMEOUT),
        })
        .await
        .map_err(DeployError)?;
        drop(key);
        if !output.ok() {
            return Err(DeployError(format!(
                "{}: delivery transfer failed: {}",
                target.name,
                host_channel::last_error_line(&output, "rsync failed")
            )));
        }
        return Ok(());
    }
    let output = runner(CommandSpec {
        argv,
        stdin,
        timeout: Some(TRANSFER_TIMEOUT),
    })
    .await
    .map_err(DeployError)?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: delivery transfer failed: {}",
            target.name,
            host_channel::last_error_line(&output, "rsync failed")
        )));
    }
    Ok(())
}

async fn commit(
    target: &ComputeTarget,
    home: &str,
    destination: &str,
    stage: &str,
    plan: &DeliveryPlan,
    runner: &Runner,
) -> Result<(), DeployError> {
    let components = plan.destination.split('/').collect::<Vec<_>>();
    let parent = Path::new(destination)
        .parent()
        .and_then(Path::to_str)
        .ok_or_else(|| DeployError("destination has no parent directory".to_string()))?;
    let name = components.last().copied().unwrap_or_default();
    let backup = format!("{parent}/.{name}.stado-previous");
    let expected_test = match plan.kind {
        SourceKind::File => "-f",
        SourceKind::Directory => "-d",
    };
    let script = format!(
        "set -eu\nreport() {{ printf '{MARKER}\\t%s\\t%s\\n' \"$1\" \"$2\"; }}\n{}\n\
         [ ! -L {destination_q} ] || {{ report refused {destination_symlink}; exit 0; }}\n\
         [ {expected_test} {stage_q} ] || {{ report failed {stage_missing}; exit 0; }}\n\
         [ -O {stage_q} ] || {{ report refused {foreign_stage}; exit 0; }}\n\
         [ ! -e {backup_q} ] && [ ! -L {backup_q} ] || {{ report refused {backup_exists}; exit 0; }}\n\
         /bin/chmod {mode:o} {stage_q}\n\
         had_previous=no\n\
         if [ -e {destination_q} ]; then /bin/mv {destination_q} {backup_q}; had_previous=yes; fi\n\
         if ! /bin/mv {stage_q} {destination_q}; then [ \"$had_previous\" = no ] || /bin/mv {backup_q} {destination_q}; report failed {rename_failed}; exit 0; fi\n\
         if [ \"$had_previous\" = yes ] && ! /bin/rm -rf -- {backup_q}; then report failed {cleanup_failed}; exit 0; fi\n\
         report delivered {destination_q}\n",
        guard_lines(home, &components, false),
        destination_q = shlex_quote(destination),
        destination_symlink = shlex_quote(&format!("destination became a symlink before commit: {destination}")),
        stage_q = shlex_quote(stage),
        stage_missing = shlex_quote(&format!("transferred {} is missing from staging", plan.kind.word())),
        foreign_stage = shlex_quote("transferred staging path is not owned by the approved account"),
        backup_q = shlex_quote(&backup),
        backup_exists = shlex_quote(&format!("previous-delivery recovery path already exists: {backup}")),
        mode = plan.root_mode,
        rename_failed = shlex_quote("could not atomically install the transferred path"),
        cleanup_failed = shlex_quote("delivery installed, but the previous path could not be removed"),
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    let (status, detail) = parse_marker(target, &output)?;
    if status != DELIVERED_STATUS {
        return Err(DeployError(format!(
            "{}: delivery {status}: {detail}",
            target.name
        )));
    }
    Ok(())
}

/// Deliver one local source to one canonical registry target.
pub async fn deliver_host(
    target_name: &str,
    source: &str,
    destination: &str,
    file_list: Option<&str>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    // Local shape and destination policy are decided before registry or host
    // contact. Host-dependent guards then run before rsync transfers a byte.
    let plan = plan(source, destination, file_list)?;
    let target = host_channel::canonical_target(target_name).await?;
    let home = host_channel::remote_home(&target, runner).await?;
    if home.bytes().any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))) {
        return Err(DeployError(format!(
            "{}: the approved account home cannot be represented safely by the rsync transport",
            target.name
        )));
    }
    let (absolute_destination, stage) = preflight(&target, &home, &plan, runner).await?;
    transfer(&target, &stage, &plan, runner).await?;
    commit(
        &target,
        &home,
        &absolute_destination,
        &stage,
        &plan,
        runner,
    )
    .await?;
    Ok(json!({
        "schema": "stado.host-delivery-receipt.v1",
        "target": target.name,
        "source": plan.source,
        "destination": format!("$HOME/{}", plan.destination),
        "kind": plan.kind.word(),
        "selection": if plan.file_list.is_some() { "nul-file-list" } else { "complete-source" },
        "status": DELIVERED_STATUS,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probierz_destination_requires_a_canonical_run_uuid_and_child() {
        assert!(destination_components(
            ".stado/work/runs/123e4567-e89b-12d3-a456-426614174000/probierz"
        )
        .is_ok());
        assert!(destination_components(".stado/work/runs/not-a-uuid/probierz").is_err());
        assert!(destination_components(
            ".stado/work/runs/123e4567-e89b-12d3-a456-426614174000"
        )
        .is_err());
    }

    #[test]
    fn destinations_outside_managed_run_root_are_refused() {
        assert!(destination_components("tmp/tree").is_err());
        assert!(destination_components("/tmp/tree").is_err());
        assert!(destination_components(".stado/work/tree").is_err());
    }
}
