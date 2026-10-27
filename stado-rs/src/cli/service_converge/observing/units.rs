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
    // A target without a services list declares no records. A record is
    // labelled by the first name key it carries; one with none is kept
    // with an empty label, which `unit_for_root` skips.
    let records = match target
        .extra
        .get(service::SERVICES_KEY)
        .and_then(Value::as_array)
    {
        Some(records) => records.as_slice(),
        None => &[],
    };
    records
        .iter()
        .filter_map(Value::as_object)
        .map(|record| {
            let label = match service::NAME_KEYS
                .iter()
                .map(|key| text(record, key))
                .find(|value| !value.is_empty())
            {
                Some(label) => label,
                None => String::new(),
            };
            (label, text(record, "path"), text(record, "kind"))
        })
        .collect()
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
    Ok((extracted.succeeded() && !program.is_empty()).then(|| program.to_string()))
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
        // A `$HOME`-relative unit path is resolved under the host's home.
        let path = match path.strip_prefix("$HOME/") {
            Some(rest) => format!("{home}/{rest}"),
            None => path.clone(),
        };
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
    let value = if printed.succeeded() {
        // `launchctl print` names the state on its own line; a unit it
        // prints without one is loaded, which is what print answering at
        // all proves.
        match printed.stdout.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "state").then(|| value.trim_start().to_string())
        }) {
            Some(state) if !state.is_empty() => state,
            Some(_) | None => "loaded".to_string(),
        }
    } else {
        // `print` refuses a unit that is not loaded; `list` then says whether
        // it is loaded without a process, running, or absent.
        let listed = host_channel::run_program(target, &["/bin/launchctl", "list"], runner).await?;
        match listed.stdout.lines().find_map(|line| {
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
        }) {
            Some(state) => state,
            None => "not-loaded".to_string(),
        }
    };
    Ok(value.replace(' ', "-"))
}

/// systemd state for one unit, or `no-systemctl` on a host without systemd.
///
/// The scope follows the unit file, as every lifecycle script decides it: a
/// unit under `/etc/systemd/system` is the system manager's, anything else
/// (`$HOME/.config/systemd/user/…`) is the account's `--user` manager. Asking
/// the system manager about a user unit answers `inactive` for a unit that
/// is running, which is what `release version show` printed for every Linux
/// host's `com.wisent.stado.service`.
async fn systemd_state(
    target: &ComputeTarget,
    runner: &Runner,
    label: &str,
    path: &str,
) -> Result<String, DeployError> {
    let found = host_channel::run_command(target, "command -v systemctl", runner).await?;
    if found.stdout.trim().is_empty() {
        return Ok("no-systemctl".to_string());
    }
    let asked = if std::path::Path::new(path).starts_with(service::SYSTEMD_UNITS) {
        host_channel::run_program(target, &["systemctl", "is-active", label], runner).await?
    } else {
        host_channel::run_command(
            target,
            &format!(
                "runtime=/run/user/$(/usr/bin/id -u); XDG_RUNTIME_DIR=\"$runtime\" \
                 DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" systemctl --user \
                 is-active {}",
                crate::deploy::shlex_quote(label)
            ),
            runner,
        )
        .await?
    };
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
        return systemd_state(target, runner, label, path).await;
    }
    if std::path::Path::new(path).starts_with(service::LAUNCHD_DAEMONS) {
        return launchd_state(target, runner, label, "system").await;
    }
    // An `id -u` that printed nothing names no GUI domain; asking `gui/`
    // would read every unit there as absent.
    let login_uid = match uid.as_deref() {
        Some(known) => known.to_string(),
        None => {
            let answered =
                host_channel::run_program(target, &["/usr/bin/id", "-u"], runner).await?;
            let read = answered.stdout.trim().to_string();
            if read.is_empty() {
                return Err(format!(
                    "the login user's uid on {} was not read: `/usr/bin/id -u` printed nothing, so {label}'s GUI domain is unknown",
                    target.name
                )
                .into());
            }
            *uid = Some(read.clone());
            read
        }
    };
    launchd_state(target, runner, label, &format!("gui/{login_uid}")).await
}
