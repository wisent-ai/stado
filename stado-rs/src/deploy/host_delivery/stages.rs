//! The three host-side stages of one delivery: the guarded preflight that
//! reserves a staging path, the rsync transfer into it, and the guarded
//! rename that commits it over the destination.

use std::path::Path;
use std::time::Duration;

use crate::deploy::{
    host_access::ssh_key, host_channel, shlex_quote, CommandSpec, DeployError, Runner,
};
use crate::targets::ComputeTarget;

use super::plan::{DeliveryPlan, SourceKind};
use super::script::{guard_lines, parse_marker, DELIVERED_STATUS, MARKER};

const TRANSFER_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub(super) async fn preflight(
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

pub(super) async fn transfer(
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
        let key = ssh_key::materialize(target.channel_key()).await?;
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

pub(super) async fn commit(
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
