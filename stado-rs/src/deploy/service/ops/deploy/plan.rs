use crate::deploy::service::*;

/// The rendered unit spellings for a deployed service, plus the label they
/// share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployPlan {
    /// The launchd label, and the stem of the systemd unit name.
    pub label: String,
    /// The systemd unit name (`<label>.service`).
    pub unit: String,
    /// Absolute program path on the target host.
    pub program: String,
    /// The argument vector the unit declares, in the one-line spelling the
    /// host reads back out of an installed unit. [`ensure_service`] compares
    /// the two, and comparing two renderings of the same list is the only way
    /// "the unit already runs this" can be a fact rather than a hope.
    pub argv: String,
    /// The launchd agent, for a host whose per-login domain exists.
    pub darwin_unit: String,
    /// The same job as a launchd daemon, for the system domain — the only one
    /// an ssh login without an Aqua session can bootstrap into. Carries
    /// [`REMOTE_USER_PLACEHOLDER`] as well as [`REMOTE_HOME_PLACEHOLDER`].
    pub darwin_daemon_unit: String,
    pub linux_unit: String,
    /// Install this plan as a system LaunchDaemon on Darwin, regardless of
    /// where the host's declaration or the per-login fallback would place it.
    /// An always-on host with no graphical session has only `system` to run a
    /// service in: its user-domain units die with the login that never comes,
    /// and a plist left in `~/Library/LaunchAgents` there is a service that
    /// runs whenever nobody needs it. `ensure_service` addresses the daemon
    /// file when this is set; the privileged steps it needs are the ones
    /// [`crate::deploy::service::ManagedService::privileged_command`] spells.
    ///
    /// Set from the target by [`requires_daemon_domain`] at plan time, so
    /// `deploy`, `ensure` and the autonomy reconciler cannot disagree about
    /// the domain of the same unit on the same host. `service ensure
    /// --as-daemon` still turns it on for a host whose declaration does not
    /// yet say always-on; nothing turns it off.
    pub force_daemon: bool,
}

/// Render every unit spelling for a new managed service.
///
/// All of them come from `local_install::InstallPlan`, the renderer used by
/// `stado bootstrap --local`. The Darwin spellings carry a reserved
/// home placeholder that the remote installer replaces before launchd reads
/// the plist; this keeps logs in the remote account's owner-only Stado directory.
const REMOTE_HOME_PLACEHOLDER: &str = "__STADO_HOME__";

/// The account a system daemon runs as, resolved on the host: a plist in
/// `/Library/LaunchDaemons` is read by root, and a job with no `UserName`
/// would run the fleet's control binary as uid 0 against an account-owned
/// `~/.stado`.
const REMOTE_USER_PLACEHOLDER: &str = "__STADO_USER__";

/// The environment every managed unit carries, before whatever its own
/// declaration adds.
///
/// `HOME` and `STADO_CONFIG` are here because launchd sets neither for a job it
/// starts, and without them a Stado process falls off the end of
/// [`crate::config_file`]'s search order — `$STADO_CONFIG`,
/// `./stado.config.json`, `~/.config/stado/config.json`, `~/.stado/config.json`
/// — and runs on defaults. A coordinator that does that ticks forever against
/// an empty store: `stado service ensure` installed
/// `com.wisent.compute.service.stado-local-control-plane` on the always-on mac
/// with `PATH` as its only variable, and eleven consecutive ticks reaped no
/// expired lease and dispatched nothing while 55 pinned jobs sat in the store
/// it could not see. The catalog-backed units on the same host
/// (`com.wisent.always-on.stado-object-api`) carried `HOME`, `STADO_CONFIG` and
/// the storage keys, so one installer produced a working unit and the other did
/// not.
///
/// Both values ride the [`REMOTE_HOME_PLACEHOLDER`] the remote installer
/// substitutes, so the account is the host's answer and never this machine's.
/// The config path is the one `stado host config-set` writes and
/// `stado host config-show` reads.
///
/// A declaration wins over all three: an entry in `extra_env` replaces the
/// value in place rather than appending a second plist key for the same name.
fn base_unit_environment(path: &str, extra_env: &[(String, String)]) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOME".to_string(), REMOTE_HOME_PLACEHOLDER.to_string()),
        (
            "STADO_CONFIG".to_string(),
            format!("{REMOTE_HOME_PLACEHOLDER}/.config/stado/config.json"),
        ),
        ("PATH".to_string(), path.to_string()),
    ];
    for (variable, value) in extra_env {
        match env.iter_mut().find(|(name, _)| name == variable) {
            Some(existing) => existing.1 = value.clone(),
            None => env.push((variable.clone(), value.clone())),
        }
    }
    env
}

pub fn plan_deploy(
    target: &ComputeTarget,
    name: &str,
    program: &str,
    args: &[String],
) -> Result<DeployPlan, DeployError> {
    validate_service_name(name)?;
    let label = local_install::label(DEPLOY_KIND, name);
    plan_deploy_labelled(target, name, &label, program, args, &[])
}

/// [`plan_deploy`] at a label the declaration already carries.
///
/// `plan_deploy` mints `com.wisent.compute.service.<name>`, which is right for
/// a unit being created and wrong for one that already exists under another
/// label. Rendering the minted spelling for a declaration that says the unit
/// is `com.wisent.stado-resolver` installs a SECOND launchd job running the
/// same program, and two resolvers competing for one stable loopback port is
/// exactly the shape of outage this module was written after. A declaration
/// that names its own label is rendered at that label, so a declared service
/// is reinstallable from the document without becoming a second service.
pub fn plan_deploy_labelled(
    target: &ComputeTarget,
    name: &str,
    label: &str,
    program: &str,
    args: &[String],
    extra_env: &[(String, String)],
) -> Result<DeployPlan, DeployError> {
    validate_service_name(name)?;
    validate_service_name(label)?;
    validate_program(program)?;
    for arg in args {
        validate_unit_argument(arg)?;
    }
    // Environment rides into the rendered unit verbatim, so it is held to the
    // same shape the runtime env-file writer enforces: an exported name and a
    // single-line value. A line break here is a second plist key, not a value.
    for (variable, value) in extra_env {
        validate_env_variable(variable)?;
        if value.contains('\n') || value.contains('\r') {
            return Err(DeployError(format!(
                "environment variable {variable} carries a line break"
            )));
        }
    }
    let label = label.to_string();
    let render = |os: LocalOs| {
        let path = match os {
            LocalOs::Darwin => "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            LocalOs::Linux => "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        };
        let mut exec_args = Vec::with_capacity(args.len() + 1);
        exec_args.push(program.to_string());
        exec_args.extend(args.iter().cloned());
        InstallPlan {
            // Both spellings are rendered here and the host picks between them
            // (`ensure_unit_path` / the remote prelude), so this plan never
            // addresses a unit path itself and needs no account.
            daemon: None,
            name: name.to_string(),
            kind: DEPLOY_KIND.to_string(),
            os,
            label: label.clone(),
            exec_args,
            env: base_unit_environment(path, extra_env),
        }
    };
    let darwin = render(LocalOs::Darwin);
    let linux = render(LocalOs::Linux);
    let unit = linux
        .unit_path(Path::new(HOME_PREFIX))
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| label.clone());
    let remote_home = Path::new(REMOTE_HOME_PLACEHOLDER);
    let mut exec_args = Vec::with_capacity(args.len() + 1);
    exec_args.push(program.to_string());
    exec_args.extend(args.iter().cloned());
    let plan = DeployPlan {
        label: label.clone(),
        unit,
        program: program.to_string(),
        argv: exec_args.join(" "),
        darwin_unit: darwin.content(remote_home),
        darwin_daemon_unit: local_install::daemon_plist_text(
            &label,
            &exec_args,
            &darwin.env,
            &remote_home
                .join(".stado")
                .join("logs")
                .join(format!("{label}.log")),
            REMOTE_USER_PLACEHOLDER,
        ),
        linux_unit: linux.content(remote_home),
        force_daemon: requires_daemon_domain(target),
    };
    guard_heredoc(&plan.darwin_unit)?;
    guard_heredoc(&plan.darwin_daemon_unit)?;
    guard_heredoc(&plan.linux_unit)?;
    Ok(plan)
}
