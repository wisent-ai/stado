//! The declared units on the host: which one runs an artefact, and what the
//! platform's service manager says about its state.

use serde_json::Value;

use crate::deploy::service;
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The services one registry target declares, as `(label, path, kind)` —
/// label falling back to unit and then to name, fields the retired probe
/// read out of the registry document with python. Used only to attribute a
/// unit to an artefact, never to decide a version.
pub(super) fn declared_service_records(target: &ComputeTarget) -> Vec<(String, String, String)> {
    let text = |record: &serde_json::Map<String, Value>, key: &str| match record.get(key) {
        Some(Value::String(value)) => value.clone(),
        _ => String::new(),
    };
    target
        .extra
        .get(service::SERVICES_KEY)
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .filter_map(Value::as_object)
                .map(|record| {
                    let label = ["label", "unit", "name"]
                        .iter()
                        .map(|key| text(record, key))
                        .find(|value| !value.is_empty())
                        .unwrap_or_default();
                    (label, text(record, "path"), text(record, "kind"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The program one declared unit runs, read out of the unit file itself.
async fn unit_program(
    target: &ComputeTarget,
    runner: &Runner,
    path: &str,
    kind: &str,
) -> Result<Option<String>, DeployError> {
    if !host_channel::remote_test(
        target,
        &format!("-f {}", crate::deploy::shlex_quote(path)),
        runner,
    )
    .await?
    {
        return Ok(None);
    }
    if kind == "systemd" {
        let read = host_channel::run_command(
            target,
            &format!(
                "sed -n 's/^ExecStart=//p' {} | head -n 1",
                crate::deploy::shlex_quote(path)
            ),
            runner,
        )
        .await?;
        return Ok(read
            .stdout
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().next())
            .map(str::to_string));
    }
    let extracted = host_channel::run_program(
        target,
        &[
            "/usr/bin/plutil",
            "-extract",
            "ProgramArguments.0",
            "raw",
            "-o",
            "-",
            path,
        ],
        runner,
    )
    .await?;
    let program = extracted.stdout.trim();
    Ok((extracted.ok() && !program.is_empty()).then(|| program.to_string()))
}

/// The declared unit whose program lives under this artefact, or nothing.
///
/// Matched on the program the unit file actually names rather than on the
/// binary's name: a label that merely mentions "stado" is a guess, and a
/// wrong unit in a report is worse than an admitted absence.
pub(super) async fn unit_for_root(
    target: &ComputeTarget,
    runner: &Runner,
    home: &str,
    services: &[(String, String, String)],
    root: &str,
) -> Result<Option<(String, String, String)>, DeployError> {
    if root.is_empty() {
        return Ok(None);
    }
    for (label, path, kind) in services {
        if label.is_empty() {
            continue;
        }
        let path = path
            .strip_prefix("$HOME/")
            .map_or_else(|| path.clone(), |rest| format!("{home}/{rest}"));
        let Some(program) = unit_program(target, runner, &path, kind).await? else {
            continue;
        };
        if program == root || program.starts_with(&format!("{root}/")) {
            return Ok(Some((label.clone(), path, kind.clone())));
        }
    }
    Ok(None)
}

/// launchd state for one label, from `launchctl print` and, when the domain
/// refuses it, from `launchctl list`. Spaces are folded to dashes so a state
/// like `spawn scheduled` stays one token.
async fn launchd_state(
    target: &ComputeTarget,
    runner: &Runner,
    label: &str,
    domain: &str,
) -> Result<String, DeployError> {
    let printed = host_channel::run_program(
        target,
        &["/bin/launchctl", "print", &format!("{domain}/{label}")],
        runner,
    )
    .await?;
    let value = if printed.ok() {
        printed
            .stdout
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "state").then(|| value.trim_start().to_string())
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "loaded".to_string())
    } else {
        let listed = host_channel::run_program(target, &["/bin/launchctl", "list"], runner).await?;
        listed
            .stdout
            .lines()
            .find_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?;
                fields.next()?;
                let name = fields.next()?;
                (name == label).then(|| {
                    if pid == "-" {
                        "loaded-not-running".to_string()
                    } else {
                        format!("running-pid-{pid}")
                    }
                })
            })
            .unwrap_or_else(|| "not-loaded".to_string())
    };
    Ok(value.replace(' ', "-"))
}

/// systemd state for one unit, or `no-systemctl` on a host without systemd.
async fn systemd_state(
    target: &ComputeTarget,
    runner: &Runner,
    label: &str,
) -> Result<String, DeployError> {
    let found = host_channel::run_command(target, "command -v systemctl", runner).await?;
    if found.stdout.trim().is_empty() {
        return Ok("no-systemctl".to_string());
    }
    let asked =
        host_channel::run_program(target, &["systemctl", "is-active", label], runner).await?;
    Ok(asked.stdout.trim().to_string())
}

/// The state of one unit, by its kind and where its unit file lives:
/// LaunchDaemons print in the system domain, everything else in the login
/// user's GUI domain.
pub(super) async fn unit_state(
    target: &ComputeTarget,
    runner: &Runner,
    label: &str,
    path: &str,
    kind: &str,
    uid: &mut Option<String>,
) -> Result<String, DeployError> {
    if kind == "systemd" {
        return systemd_state(target, runner, label).await;
    }
    if path.starts_with("/Library/LaunchDaemons/") {
        return launchd_state(target, runner, label, "system").await;
    }
    if uid.is_none() {
        let answered = host_channel::run_program(target, &["/usr/bin/id", "-u"], runner).await?;
        *uid = Some(answered.stdout.trim().to_string());
    }
    launchd_state(
        target,
        runner,
        label,
        &format!("gui/{}", uid.as_deref().unwrap_or_default()),
    )
    .await
}
