//! Fleet-owned gVisor sandboxes on registered Linux hosts.
use crate::cli::CmdError;
use crate::deploy::{host_channel, shlex_quote};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use std::num::{NonZeroU32, NonZeroU64};
mod build;
mod egress;
mod passed;
mod programs;

#[derive(Args)]
pub struct Host {
    /// Registered Linux host; no cloud machine is rented.
    #[arg(long)]
    host: String,
}
#[derive(Args)]
pub struct Named {
    #[command(flatten)]
    host: Host,
    /// Lowercase letters, digits and hyphens, beginning with a letter.
    name: String,
}
#[derive(Subcommand)]
pub enum FleetCommands {
    /// Read Docker, gVisor and cgroup prerequisites without changing the host.
    Inventory(Host),
    /// Read managed sandboxes, never unrelated containers.
    List(Host),
    /// Create a sandbox with caller-declared enforced resource limits.
    Create {
        #[command(flatten)]
        named: Named,
        #[command(flatten)]
        spec: build::Spec,
    },
    /// Change CPU, RAM and process ceilings and read back the enforced values.
    Update {
        #[command(flatten)]
        named: Named,
        #[arg(long)]
        cpus: NonZeroU32,
        #[arg(long)]
        memory_bytes: NonZeroU64,
        #[arg(long)]
        pids_limit: NonZeroU32,
    },
    /// Read observed state, security settings and resource limits.
    Show(Named),
    /// Read what the running sandbox has used since its last start: CPU
    /// time, peak and current memory, processes and workspace size.
    Usage(Named),
    /// Execute inside the sandbox, optionally detached with retained output.
    Exec {
        #[command(flatten)]
        named: Named,
        #[arg(long)]
        command: String,
        #[arg(long)]
        detach: bool,
        /// Credential fields the command alone receives in its environment
        /// (`--credential NAME=ITEM#FIELD`, optionally under a consumer's
        /// grant); their values are printed nowhere.
        #[command(flatten)]
        credentials: passed::Credentials,
    },
    /// Read a detached execution's persisted output and completion status.
    Logs {
        #[command(flatten)]
        named: Named,
        #[arg(long)]
        execution: String,
    },
    /// Stop execution while preserving the private workspace.
    Stop(Named),
    /// Resume the same sandbox, limits and workspace.
    Resume(Named),
    /// Make a new sandbox from a stopped one: the same image, enforced
    /// limits and lifetime, and a copy of its workspace kept only when every
    /// file's name and content match.
    Fork {
        #[command(flatten)]
        named: Named,
        /// Name of the new sandbox.
        #[arg(long)]
        to: String,
    },
    /// Delete the managed sandbox and private workspace, confirming absence.
    Delete {
        #[command(flatten)]
        named: Named,
        #[arg(long)]
        confirm: String,
    },
}
use build::valid_name;
pub async fn dispatch(command: FleetCommands) -> Result<(), CmdError> {
    let (host, operation, script) = prepare(command).await?;
    let registry = host_channel::canonical_registry().await?;
    let target = host_channel::resolve_target(&registry, &host)?;
    if !target.release_platform.starts_with("linux-") {
        return Err(CmdError::refused(format!("{host}: fleet sandboxes require Linux with Docker, cgroup v2 and gVisor runsc; observed platform {}", target.release_platform)));
    }
    let runner = crate::deploy::production_runner();
    let output = host_channel::run_script(target, &script, &runner).await?;
    let mut result: Option<Value> = output
        .stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("STADO_SANDBOX\t"))
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| {
            CmdError::refused(format!(
                "{host}: {operation} returned an unreadable sandbox receipt: {error}"
            ))
        })?;
    if operation == "list" && output.succeeded() {
        let sandboxes = output
            .stdout
            .lines()
            .filter(|line| !line.starts_with("STADO_SANDBOX\t") && !line.trim().is_empty())
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                CmdError::refused(format!(
                    "{host}: Docker returned an unreadable sandbox listing: {error}"
                ))
            })?;
        result = Some(json!({"sandboxes":sandboxes}));
    }
    let kernel_limits = output
        .stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("STADO_SANDBOX_LIMITS\t"))
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|error| {
            CmdError::refused(format!(
                "{host}: {operation} returned unreadable kernel resource observations: {error}"
            ))
        })?;
    let report = json!({"schema":"stado.fleet-sandbox.v1", "host":host,
        "operation":operation, "ok":output.succeeded(), "exit_code":output.code,
        "stdout":output.stdout, "stderr":output.stderr, "result":result,
        "kernel_limits":kernel_limits});
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !output.succeeded() {
        return Err(CmdError::refused(format!(
            "{host}: sandbox {operation} failed with exit {}; stderr: {}; stdout: {}",
            output.code,
            output.stderr.trim(),
            output.stdout.trim()
        )));
    }
    if result.is_none() {
        return Err(CmdError::refused(format!(
            "{host}: sandbox {operation} exited without an observed-state receipt"
        )));
    }
    Ok(())
}
async fn prepare(command: FleetCommands) -> Result<(String, &'static str, String), CmdError> {
    let (host, name, operation, body) = match command {
        FleetCommands::Inventory(host) => (
            host.host,
            None,
            "inventory",
            programs::inventory().to_string(),
        ),
        FleetCommands::List(host) => (host.host, None, "list", programs::list().to_string()),
        FleetCommands::Create { named, spec } => {
            (named.host.host, Some(named.name), "create", spec.program()?)
        }
        FleetCommands::Update {
            named,
            cpus,
            memory_bytes,
            pids_limit,
        } => (
            named.host.host,
            Some(named.name),
            "update",
            programs::update(cpus, memory_bytes, pids_limit),
        ),
        FleetCommands::Show(named) => (
            named.host.host,
            Some(named.name),
            "show",
            programs::show().to_string(),
        ),
        FleetCommands::Usage(named) => (
            named.host.host,
            Some(named.name),
            "usage",
            programs::usage().to_string(),
        ),
        FleetCommands::Exec {
            named,
            command,
            detach,
            credentials,
        } => {
            if command.trim().is_empty() {
                return Err(CmdError::usage("--command must not be empty"));
            }
            let passed = credentials.values().await?;
            (
                named.host.host,
                Some(named.name),
                "exec",
                programs::execute(&command, detach, &passed),
            )
        }
        FleetCommands::Logs { named, execution } => {
            let execution = uuid::Uuid::parse_str(&execution)
                .map_err(|error| {
                    CmdError::usage(format!(
                        "--execution must be the UUID returned by exec --detach: {error}"
                    ))
                })?
                .to_string();
            (
                named.host.host,
                Some(named.name),
                "logs",
                programs::logs(&execution),
            )
        }
        FleetCommands::Stop(named) => (
            named.host.host,
            Some(named.name),
            "stop",
            programs::transition(false),
        ),
        FleetCommands::Resume(named) => (
            named.host.host,
            Some(named.name),
            "resume",
            programs::transition(true),
        ),
        FleetCommands::Delete { named, confirm } => {
            if confirm != named.name {
                return Err(CmdError::usage(
                    "--confirm must equal the sandbox name; nothing was deleted",
                ));
            }
            (
                named.host.host,
                Some(named.name),
                "delete",
                programs::delete().to_string(),
            )
        }
        FleetCommands::Fork { named, to } => {
            if !valid_name(&to) || to == named.name {
                return Err(CmdError::usage("--to must name a new sandbox other than the source: a lowercase letter, then lowercase letters, digits and hyphens; nothing was forked"));
            }
            (named.host.host, Some(named.name), "fork", build::fork(&to))
        }
    };
    if let Some(name) = &name {
        if !valid_name(name) {
            return Err(CmdError::usage("sandbox names must begin with a lowercase letter and contain only lowercase letters, digits and hyphens"));
        }
    }
    // A named operation's program reads the sandbox's name, container and
    // volume; one that names no sandbox (list) declares none of them.
    let header = match &name {
        Some(name) => format!(
            "set -euo pipefail\nname={}\ncontainer=stado-sandbox-$name\nvolume=stado-sandbox-$name\n",
            shlex_quote(name)
        ),
        None => "set -euo pipefail\n".to_string(),
    };
    Ok((
        host,
        operation,
        format!("{header}{}\n{body}", programs::PRELUDE),
    ))
}
