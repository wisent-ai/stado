//! `stado service ...` — the full service-management layer.
//!
//! NO Python original: the Python CLI stops at `host recover`, and that gap
//! is the point. `docs/missing-commands.md` items seven through fourteen
//! were written after a wedged `com.wisent.weles-api` sat unmanaged on a
//! mac mini: the unit existed on the host, Stado did not know about it, and
//! there was no command to list it, restart it or adopt it.
//!
//! The engine is [`crate::deploy::service`]; this module is the operator
//! surface over it. Two properties are worth keeping when editing:
//!
//! - `list` answers from the health beacons alone. No ssh, no per-host
//!   round trip, so the fleet-wide question stays answerable when a host
//!   is the thing that is broken. `status` answers the same way, and adds
//!   best-effort host reads — launchd's last exit status and the stderr
//!   tail — only for units whose beacon state is `failed`; those reads
//!   degrade to a note, never to a failed command.
//! - `adopt`, `retire` and `deploy` mutate the canonical registry through
//!   `cli/registry.rs::{fetch_document, push_document}` — the validated
//!   write path — and never hand-edit the document. `push_document`
//!   validates before it writes, so a mutation that would produce an
//!   invalid registry is refused with nothing uploaded.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::deploy::service::{
    self, ManagedService, ServiceEnv, ServiceLog, ServiceStatus, UnitDomain, SOURCE_RECOVERY,
    SOURCE_REGISTRY,
};
use crate::deploy::{host_channel, host_exec, production_runner, DeployError};
use crate::observations;
use crate::queue::JobStorage;
use crate::targets;

use super::{registry, table, CmdError};

#[derive(Subcommand)]
pub enum ServiceCommands {
    /// Where a service is reachable from here, and who may use it.
    #[command(subcommand)]
    Directory(crate::cli::directory::DirectoryCommands),

    /// The preconfigured Wisent services, ready to deploy by name: no
    /// declaration to write, no flags to know. `service deploy <name>` and
    /// `service ensure <name>` resolve these when nothing else declares the
    /// unit.
    Catalog {
        #[arg(long)]
        json: bool,
    },

    /// Every registry-managed service across all hosts, with its state.
    ///
    /// Answered from the latest health beacons, so it costs no ssh and
    /// reports on hosts that are not currently reachable. A host that has
    /// published no beacon reports `unknown`, which is deliberately not
    /// the same answer as `missing`.
    ///
    /// `OBSERVED` is a different question from `STATE` and is answered by a
    /// different party. `STATE` is what the host says about its own unit;
    /// `OBSERVED` is when anybody last went and looked at the service from
    /// outside. A host with a closed lid publishes no beacon and says
    /// nothing, so `STATE` goes quiet rather than wrong -- and quiet is what
    /// read as fine for twelve days. `never` in this column means no machine
    /// has ever confirmed this service from any vantage.
    ///
    /// `--unowned` answers the opposite question: which product processes are
    /// running that no launchd job or systemd unit owns. Two `stado agent`
    /// processes ran that way for four days, executing a binary older than the
    /// one on disk, and every answer in this group was about declared units
    /// and so said nothing about them.
    List {
        /// Report the product processes no unit owns instead of the declared
        /// managed set. This is the one question in this group the beacons
        /// cannot answer -- an unowned process is by definition in nobody's
        /// declaration -- so it costs one read-only ssh per kind=local host.
        #[arg(long)]
        unowned: bool,
        #[arg(long)]
        json: bool,
    },

    /// Go to each consumer and check the endpoint it is told to use.
    ///
    /// `list` reports what hosts say about their units. This reports whether
    /// the directory's addresses answer, from the machines that must call
    /// them -- the one question every other check in this binary skips.
    /// States are `observed`, `unreachable`, and `unverified` for a probe
    /// that could not run; the third is never folded into the other two.
    /// Exits non-zero only on `unreachable`, so an uninstalled probe cannot
    /// masquerade as an outage.
    Verify {
        /// Check one host's declarations instead of the whole fleet.
        #[arg(long)]
        host: Option<String>,
        /// Probe from this machine only, without using the fleet channel.
        /// This is the mode the installed probe helper runs.
        #[arg(long)]
        local: bool,
        #[arg(long)]
        json: bool,
    },

    /// Is the host running the version the registry declares for it?
    ///
    /// `list` and `show` answer questions about the unit -- loaded, running,
    /// which program -- and every one of those answers stays true across a
    /// release that never reached the box. This compares
    /// `targets[].managed_versions`, the declared version of each managed
    /// binary on TARGET, against the version that host actually runs.
    ///
    /// Verdicts are `in-sync`, `drifted`, and `unknown` for a binary whose
    /// installed version could not be read; the third is never folded into
    /// either of the other two. Reporting exits non-zero on `drifted` alone,
    /// so an uninstalled reporting helper cannot masquerade as drift.
    /// `--apply` delivers the declared version of every drifted binary through
    /// `stado host release`, re-reads the installed versions afterwards, and
    /// exits non-zero unless every binary in scope is confirmed `in-sync`.
    Converge {
        /// Registry host to compare against its own declarations.
        target: String,
        /// One managed binary by name; omit for every binary TARGET declares
        /// a version for.
        binary: Option<String>,
        /// Deliver the declared version of every drifted binary, then read the
        /// installed versions back.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },

    /// Registry-managed services carrying Echo onboarding product metadata.
    ///
    /// Emits the versioned JSON envelope accepted by Echo's Stado catalog
    /// synchronization endpoint.
    OnboardingCatalog,

    /// One service's state everywhere it is managed.
    Status {
        /// Service name, or the host's own name for the unit.
        name: String,
        #[arg(long)]
        json: bool,
    },

    /// Restart one managed unit, without a full host-recovery pass.
    Restart {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to restart it everywhere it
        /// is managed.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Move an already-managed service onto a new artifact version.
    ///
    /// `deploy` installs a unit that is not yet managed and refuses to touch one
    /// that is. This is the other half: the unit is left exactly as it is, the
    /// new version is placed beside the running one and `current` is relinked,
    /// so the change takes effect on the next restart and a rollback is a
    /// relink rather than a redeploy.
    Update {
        /// Service name as the registry manages it.
        name: String,
        #[arg(long)]
        host: String,
        /// Published artifact to install.
        #[arg(long, conflicts_with = "from_archive")]
        from_artifact: Option<String>,
        /// Local release archive to install, for a bundle that no object store
        /// the fleet shares is carrying yet.
        #[arg(long, conflicts_with = "from_artifact")]
        from_archive: Option<String>,
        /// Point `current` back at a version directory already on the host.
        #[arg(long, conflicts_with_all = ["from_artifact", "from_archive"])]
        rollback_to: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// What a managed unit actually runs: its program, arguments and unit file.
    ///
    /// `env` answers what the unit runs *with*; nothing answered what it runs.
    /// That gap is why a restart that dropped every argument after the program
    /// path looked like a broken service rather than a broken restart.
    Show {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Stop one managed unit, including a process the unit no longer owns.
    ///
    /// `retire` removes a service from management; this only stops it. The
    /// difference matters when a restart has previously spawned the program
    /// outside its own label: launchctl then disowns it, the stale process
    /// keeps the port, and every later restart dies on "address already in
    /// use" while the broken instance serves on.
    Stop {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to stop it everywhere it is managed.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Synchronize one Skarbiec field into a service's runtime env file.
    ///
    /// The value is read through the isolated service-verifier grant and carried
    /// in the SSH request body. It is never printed or placed in argv.
    SecretSync {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to update.
        #[arg(long)]
        host: String,
        /// Skarbiec item containing the secret.
        #[arg(long)]
        item: String,
        /// Exact string field in the Skarbiec item.
        #[arg(long, default_value = "token")]
        field: String,
        /// Environment variable to replace.
        #[arg(long)]
        variable: String,
        /// Runtime env file on the target, absolute or rooted at $HOME.
        #[arg(long)]
        env_file: String,
        /// Restart the service after a successful atomic sync.
        #[arg(long)]
        restart: bool,
        #[arg(long)]
        json: bool,
    },

    /// Verify a managed service's bearer against a read-only loopback endpoint.
    ///
    /// With `--repair`, a failed check atomically synchronizes the secret,
    /// restarts the unit, and checks the endpoint once more.
    AuthCheck {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// The single registry host to check.
        #[arg(long)]
        host: String,
        /// Skarbiec item containing the bearer.
        #[arg(long)]
        item: String,
        /// Exact string field in the Skarbiec item.
        #[arg(long, default_value = "token")]
        field: String,
        /// Read-only loopback HTTP endpoint that requires authentication.
        #[arg(long)]
        url: String,
        /// Send an empty JSON POST instead of a GET; useful for auth-first APIs.
        #[arg(long)]
        post_empty_json: bool,
        /// Treat this exact HTTP status as proof that authentication passed.
        #[arg(long)]
        expect_status: Option<u16>,
        /// On failure, synchronize the secret, restart, and check again.
        #[arg(long)]
        repair: bool,
        /// If repair still fails, stop the unmanaged process owning the URL port.
        #[arg(long, requires = "repair")]
        take_over_listener: bool,
        /// Environment variable to replace when repairing.
        #[arg(long, requires = "repair")]
        variable: Option<String>,
        /// Runtime env file to update when repairing.
        #[arg(long, requires = "repair")]
        env_file: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Bring an existing launchd/systemd unit under management.
    ///
    /// The unit must already exist on the host — adoption claims what is
    /// there, it does not create anything. The host is probed first and the
    /// registry records what the host reported, not what was assumed.
    Adopt {
        /// launchd label or systemd unit name, as the host knows it.
        unit: String,
        /// Explicit registry host that runs it.
        #[arg(
            long,
            conflicts_with = "host_heuristic",
            required_unless_present = "host_heuristic"
        )]
        host: Option<String>,
        /// Declarative placement selector resolved against the registry.
        #[arg(long, conflicts_with = "host")]
        host_heuristic: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Attach central onboarding product metadata to a managed service.
    ///
    /// The metadata becomes part of the canonical Stado registry and is
    /// emitted by `service list --json` for Echo catalog synchronization.
    Onboarding {
        /// Service name, launchd label, or systemd unit.
        name: String,
        /// Registry host that declares the service.
        #[arg(long)]
        host: String,
        #[arg(long)]
        product_id: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        repository: String,
        #[arg(long, value_delimiter = ',', num_args = 1..)]
        surfaces: Vec<String>,
        #[arg(long)]
        first_success_fact: String,
        #[arg(long, default_value = "both")]
        onboarding_kind: String,
        #[arg(long, default_value = "active")]
        status: String,
        #[arg(long)]
        json: bool,
    },

    /// Remove a service from management: bootout/disable and forget.
    ///
    /// Unit files are left on disk. Retiring is a management decision, not
    /// a deletion.
    Retire {
        /// launchd label or systemd unit name, as the host knows it.
        unit: String,
        /// Registry host that runs it.
        #[arg(long)]
        host: String,
        #[arg(long)]
        json: bool,
    },

    /// Remove a service entirely: stop it, forget it, and delete its unit
    /// file from the host — the operation an operator means by "remove this
    /// service", which `retire` deliberately is not. The file path comes from
    /// the registry declaration, never from operator words. Refuses before
    /// anything moves when the unit cannot be stopped; a file the channel
    /// may not delete leaves the service retired and says so, with the
    /// privileged command that could remove it.
    Remove {
        /// launchd label or systemd unit name, as the host knows it.
        unit: String,
        /// Registry host that runs it.
        #[arg(long)]
        host: String,
        #[arg(long)]
        json: bool,
    },

    /// Install a new unit under management: render, push, bootstrap,
    /// record.
    Deploy {
        /// Service name; lowercase letters, digits, '.', '-' and '_'.
        name: String,
        /// Explicit registry host to install it on.
        #[arg(
            long,
            conflicts_with = "host_heuristic",
            required_unless_present = "host_heuristic"
        )]
        host: Option<String>,
        /// Declarative placement selector resolved against the registry.
        #[arg(long, conflicts_with = "host")]
        host_heuristic: Option<String>,
        /// Absolute path, ON THE TARGET HOST, of the program the unit runs.
        /// The plist / systemd unit is rendered around it by the same
        /// renderer `stado bootstrap --local` uses.
        #[arg(long, conflicts_with = "from_artifact")]
        from: Option<String>,
        /// Published artifact to install and run instead of a path already on
        /// the host. The reference is resolved to an immutable version, that
        /// version is placed under ~/.stado/services/NAME/<version>/, its
        /// declared sha256 is verified there, and `current` is moved onto it.
        /// The unit runs through `current`, so a later install or a rollback
        /// is a relink rather than a redeploy.
        #[arg(long = "from-artifact")]
        from_artifact: Option<String>,
        /// One argument the unit is started with; repeat for each. A program
        /// that needs a subcommand or a port to be the service it is named
        /// after cannot be deployed without these, and hand-starting it
        /// beside the unit is how a host ends up serving on a port no
        /// declaration mentions.
        #[arg(long = "arg")]
        args: Vec<String>,
        #[arg(long)]
        json: bool,
    },

    /// Declare a service against the fleet's one contract.
    ///
    /// Stado ships no list of services: a service is whatever its author
    /// declares — an immutable source the bytes come from, a run spec the
    /// unit is rendered from, how the service is observed, and who may call
    /// it. This command writes that declaration into the service directory;
    /// `deploy` then needs no flags beyond the name, because everything it
    /// would ask for is already written down.
    Declare {
        /// Path to the declaration file (JSON). Required keys: `name`,
        /// `host`, `source.artifact`, `source.sha256`. Optional: `run`,
        /// `verify`, `consumers`, `endpoints`, or `port` as a shorthand for
        /// one loopback endpoint on the declared host.
        #[arg(long)]
        file: String,
        #[arg(long)]
        json: bool,
    },

    /// Assert the unit a host must be running, over ssh, idempotently.
    ///
    /// `deploy` installs a unit and refuses one that is already declared, so
    /// there was no command an operator could run twice, or run from a script,
    /// to make a host run what it is supposed to run. This one reads what is
    /// there first: a unit already running the declared program is reported
    /// `already_correct` with nothing touched, a unit that exists but is not
    /// running is kicked in place, and a host with no unit gets one.
    ///
    /// It also works where `deploy` cannot. An ssh login has no Aqua session,
    /// `launchctl bootstrap gui/$uid` answers `Could not switch to audit
    /// session ... Operation not permitted`, and `deploy` returned that having
    /// installed nothing — which is how two `stado agent` processes came to run
    /// for four days with no unit behind them. Where the per-login domain does
    /// not exist, the unit is rendered for launchd's system domain and
    /// installed as a daemon in /Library/LaunchDaemons.
    ///
    /// An existing unit is only ever restarted in place (`kickstart -k`), never
    /// unloaded and bootstrapped back: that sequence took the always-on host
    /// down once already. A loaded unit whose definition names a different
    /// program is refused rather than overwritten, because launchd holds the
    /// definition it bootstrapped and a rewritten file under a live job changes
    /// nothing an operator can see.
    Ensure {
        /// Service name; lowercase letters, digits, '.', '-' and '_'.
        name: String,
        /// The single registry host that must be running it.
        #[arg(long)]
        host: String,
        /// Absolute path, ON THE TARGET HOST, of the program the unit runs.
        /// Omit it to render the unit from the service's own declaration,
        /// which is what makes a declared service reinstallable from the
        /// document instead of from a plist somebody installed by hand.
        #[arg(long)]
        from: Option<String>,
        /// One argument the unit is started with; repeat for each. Only with
        /// `--from`: the declared argument vector belongs to the declared
        /// program and the two are never mixed.
        #[arg(long = "arg")]
        args: Vec<String>,
        /// Why this host must run this unit. Required: `ensure` installs units
        /// and restarts running ones, and every such change is recorded beside
        /// the registry document it declared the unit in.
        #[arg(long)]
        reason: String,
        #[arg(long)]
        json: bool,
    },

    /// Tail a managed unit's log over the approved channel.
    Logs {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to tail every host that
        /// manages it.
        #[arg(long)]
        host: Option<String>,
        /// Lines of tail to fetch.
        #[arg(long, default_value_t = default_log_lines())]
        lines: usize,
        #[arg(long)]
        json: bool,
    },

    /// The effective environment a managed unit runs with, secrets
    /// redacted.
    ///
    /// Parsed from the unit's own plist / systemd unit file. Values whose
    /// variable name looks like a credential are replaced, in the table and
    /// in `--json` alike.
    Env {
        /// Service name, or the host's own name for the unit.
        name: String,
        /// Restrict to one registry host; omit to read every host that
        /// manages it.
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// Default `--lines` for `service logs`: one byte's worth of lines. Derived
/// from `u8::MAX` rather than written as a number, the same way
/// `cli/mod.rs::default_mail_results` derives its default from `u8::BITS`.
fn default_log_lines() -> usize {
    usize::from(u8::MAX)
}

pub async fn dispatch(command: ServiceCommands) -> Result<(), CmdError> {
    match command {
        ServiceCommands::Directory(sub) => crate::cli::directory::dispatch(sub).await,
        ServiceCommands::Catalog { json } => catalog(json).await,
        ServiceCommands::List { unowned, json } => {
            if unowned {
                list_unowned(json).await
            } else {
                list(json).await
            }
        }
        ServiceCommands::Verify { host, local, json } => {
            if local {
                crate::cli::service_verify::verify_local(json).await
            } else {
                crate::cli::service_verify::verify(host.as_deref(), json).await
            }
        }
        ServiceCommands::Converge {
            target,
            binary,
            apply,
            json,
        } => crate::cli::service_converge::converge(&target, binary.as_deref(), apply, json).await,
        ServiceCommands::OnboardingCatalog => onboarding_catalog().await,
        ServiceCommands::Status { name, json } => status(&name, json).await,
        ServiceCommands::Update {
            name,
            host,
            from_artifact,
            from_archive,
            rollback_to,
            json,
        } => {
            update(
                &name,
                &host,
                from_artifact.as_deref(),
                from_archive.as_deref(),
                rollback_to.as_deref(),
                json,
            )
            .await
        }
        ServiceCommands::Show { name, host, json } => show(&name, host.as_deref(), json).await,
        ServiceCommands::Stop { name, host, json } => stop(&name, host.as_deref(), json).await,
        ServiceCommands::Restart { name, host, json } => {
            restart(&name, host.as_deref(), json).await
        }
        ServiceCommands::SecretSync {
            name,
            host,
            item,
            field,
            variable,
            env_file,
            restart,
            json,
        } => {
            secret_sync(SecretSyncOptions {
                name: &name,
                host: &host,
                item: &item,
                field: &field,
                variable: &variable,
                env_file: &env_file,
                restart_after_sync: restart,
                as_json: json,
            })
            .await
        }
        ServiceCommands::AuthCheck {
            name,
            host,
            item,
            field,
            url,
            repair,
            take_over_listener,
            post_empty_json,
            expect_status,
            variable,
            env_file,
            json,
        } => {
            auth_check(AuthCheckOptions {
                name: &name,
                host: &host,
                item: &item,
                field: &field,
                url: &url,
                post_empty_json,
                expect_status,
                repair,
                take_over_listener,
                variable: variable.as_deref(),
                env_file: env_file.as_deref(),
                as_json: json,
            })
            .await
        }
        ServiceCommands::Adopt {
            unit,
            host,
            host_heuristic,
            json,
        } => adopt(&unit, host.as_deref(), host_heuristic.as_deref(), json).await,
        ServiceCommands::Onboarding {
            name,
            host,
            product_id,
            display_name,
            repository,
            surfaces,
            first_success_fact,
            onboarding_kind,
            status,
            json,
        } => {
            onboarding(OnboardingOptions {
                name: &name,
                host: &host,
                product_id: &product_id,
                display_name: &display_name,
                repository: &repository,
                surfaces,
                first_success_fact: &first_success_fact,
                onboarding_kind: &onboarding_kind,
                status: &status,
                as_json: json,
            })
            .await
        }
        ServiceCommands::Retire { unit, host, json } => retire(&unit, &host, json).await,
        ServiceCommands::Remove { unit, host, json } => remove(&unit, &host, json).await,
        ServiceCommands::Deploy {
            name,
            host,
            host_heuristic,
            from,
            from_artifact,
            args,
            json,
        } => {
            deploy(
                &name,
                host.as_deref(),
                host_heuristic.as_deref(),
                from,
                from_artifact,
                &args,
                json,
            )
            .await
        }
        ServiceCommands::Declare { file, json } => declare(&file, json).await,
        ServiceCommands::Ensure {
            name,
            host,
            from,
            args,
            reason,
            json,
        } => {
            ensure(EnsureOptions {
                name: &name,
                host: &host,
                from: from.as_deref(),
                args: &args,
                reason: &reason,
                as_json: json,
            })
            .await
        }
        ServiceCommands::Logs {
            name,
            host,
            lines,
            json,
        } => logs(&name, host.as_deref(), lines, json).await,
        ServiceCommands::Env { name, host, json } => env(&name, host.as_deref(), json).await,
    }
}

// ---------------------------------------------------------------------------
// Shared resolution
// ---------------------------------------------------------------------------

fn click(exc: DeployError) -> CmdError {
    CmdError::click(exc.to_string())
}
async fn resolve_placement(
    host: Option<&str>,
    host_heuristic: Option<&str>,
) -> Result<(crate::targets::ComputeTarget, Option<String>), CmdError> {
    let resolved_host = if let Some(host) = host {
        host.to_string()
    } else if let Some(heuristic) = host_heuristic {
        let registry = targets::load_registry_auto()
            .await
            .map_err(|exc| CmdError::click(exc.to_string()))?;
        registry
            .lookup_host_heuristic(heuristic)
            .map(|target| target.name.clone())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "host heuristic '{heuristic}' matches no local registry target"
                ))
            })?
    } else {
        return Err(CmdError::click(
            "either --host or --host-heuristic is required".to_string(),
        ));
    };
    let target = host_channel::canonical_target(&resolved_host)
        .await
        .map_err(click)?;
    Ok((target, host_heuristic.map(str::to_string)))
}

/// Reuse the host command's provider-neutral beacon store selection.
async fn beacon_store() -> Result<JobStorage, CmdError> {
    super::host::beacon_store().await
}

/// The declared managed set matching NAME, without touching beacons.
///
/// The write-side commands need the declaration — its unit id and its
/// unit-file path — not its state, so they must not pay for a beacon read
/// per host to get it.
pub(crate) async fn declared_matching(
    name: &str,
    host: Option<&str>,
) -> Result<Vec<ManagedService>, CmdError> {
    if let Some(host) = host {
        // Resolve the host first so an unknown or non-local target reports
        // the registry's own precise refusal rather than "no such service".
        host_channel::canonical_target(host).await.map_err(click)?;
    }
    let registry = registry::read_registry().await?;
    let mut found: Vec<ManagedService> = Vec::new();
    for target in registry.local_targets() {
        if host.is_some_and(|host| target.name != host) {
            continue;
        }
        found.extend(
            service::declared_services(target)
                .into_iter()
                .filter(|declared| declared.matches(name)),
        );
    }
    if found.is_empty() {
        return Err(unmanaged(name, host));
    }
    Ok(found)
}

fn unmanaged(name: &str, host: Option<&str>) -> CmdError {
    match host {
        Some(host) => CmdError::click(format!(
            "{name} is not a registry-managed service on {host}"
        )),
        None => CmdError::click(format!("no registry-managed service named {name}")),
    }
}

/// `-` for an empty cell, the spelling `monitor/host_health.rs` already
/// prints for a beacon field it does not have.
fn dash(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn print_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Read commands
// ---------------------------------------------------------------------------

async fn list(json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::list_services(&store).await.map_err(click)?;
    render_status(&rows, json, &[])
}

/// `service list --unowned` — product processes no unit owns, fleet-wide.
///
/// The one read in this group that cannot come off the beacons: a beacon
/// reports the units the host was told about, and an unowned process is by
/// construction in nobody's declaration. So it is one read-only ssh per
/// kind=local host, and a host that will not answer is named on stderr rather
/// than dropped — "no unowned processes" and "nobody looked" are the fold this
/// whole group refuses to make.
async fn list_unowned(json: bool) -> Result<(), CmdError> {
    let registry = registry::read_registry().await?;
    let runner = production_runner();
    let mut found: Vec<service::UnownedProcess> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for target in registry.local_targets() {
        match service::unowned_processes(target, &runner).await {
            Ok(processes) => found.extend(processes),
            Err(exc) => failures.push(format!("{}: {exc}", target.name)),
        }
    }
    if json {
        let payload: Vec<Value> = found.iter().map(service::UnownedProcess::to_json).collect();
        print_json(&json!({"unowned": payload}))?;
    } else {
        let cells: Vec<Vec<String>> = found
            .iter()
            .map(|process| {
                vec![
                    process.host.clone(),
                    process.pid.clone(),
                    process.product_guess(),
                    dash(&process.started_at),
                    process.command.clone(),
                ]
            })
            .collect();
        table::print(
            &["HOST", "PID", "PRODUCT_GUESS", "STARTED_AT", "COMMAND"],
            &cells,
        );
    }
    fail_if_any(&failures, "scan for unowned processes")
}

async fn onboarding_catalog() -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::list_services(&store).await.map_err(click)?;
    let services: Vec<Value> = rows
        .iter()
        .filter(|row| row.service.source == SOURCE_REGISTRY && row.service.onboarding.is_some())
        .map(ServiceStatus::to_json)
        .collect();
    print_json(&json!({"schema_version": 1, "services": services}))
}

async fn status(name: &str, json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::find_services(&store, name).await.map_err(click)?;
    if rows.is_empty() {
        return Err(unmanaged(name, None));
    }
    // A `failed` row is the beacon saying "it died" — more often than not
    // with an empty detail. The why lives on the host: launchd's last exit
    // status, and the stderr the unit wrote before it went. Gather it
    // best-effort over the read-only channels; `status` must still answer
    // when the host cannot.
    let runner = production_runner();
    let mut failures: Vec<FailureEvidence> = Vec::new();
    for row in &rows {
        if row.state == service::STATE_FAILED {
            failures.push(failure_evidence(row, &runner).await);
        }
    }
    render_status(&rows, json, &failures)
}

/// How many stderr lines one `failure:` block may carry.
const FAILURE_STDERR_LINES: usize = 10;

/// Why one `failed` unit died, gathered best-effort from the host itself.
/// Every read can fail — the host may be the thing that is broken — so a
/// failed read degrades to a note, never to a failed `status`.
struct FailureEvidence {
    host: String,
    unit: String,
    /// launchd's last exit status for the label, when `launchctl list`
    /// carried it.
    last_exit: Option<String>,
    /// Where the stderr tail came from, or the reason there is none.
    error_origin: Option<String>,
    error_lines: Vec<String>,
    /// Why gathering failed, when it did.
    note: Option<String>,
}

impl FailureEvidence {
    fn push_note(&mut self, note: String) {
        match &mut self.note {
            Some(existing) => {
                existing.push_str("; ");
                existing.push_str(&note);
            }
            None => self.note = Some(note),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "last_exit": self.last_exit,
            "error_origin": self.error_origin,
            "error_lines": self.error_lines,
            "note": self.note,
        })
    }
}

/// The `Status` column of one label in `launchctl list` output: launchd's
/// last exit status for the job while nothing runs under it. Columns are
/// PID, Status, Label, tab-separated; the header row never collides with a
/// real label.
fn launchctl_last_exit(stdout: &str, label: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let mut columns = line.split('\t');
        let _pid = columns.next()?;
        let status = columns.next()?;
        let name = columns.next()?;
        (name == label).then(|| status.to_string())
    })
}

async fn failure_evidence(row: &ServiceStatus, runner: &crate::deploy::Runner) -> FailureEvidence {
    let unit = row.service.unit_id().to_string();
    let mut evidence = FailureEvidence {
        host: row.service.host.clone(),
        unit: unit.clone(),
        last_exit: None,
        error_origin: None,
        error_lines: Vec::new(),
        note: None,
    };
    // The last exit status rides the approved read-only allowlist: the
    // exact `launchctl list` entry, never a shell.
    let words = vec!["launchctl".to_string(), "list".to_string()];
    match host_exec::exec_host(&row.service.host, &words, runner).await {
        Ok(report)
            if report.get("status").and_then(Value::as_str) == Some(host_exec::OK_STATUS) =>
        {
            let stdout = report
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default();
            evidence.last_exit = launchctl_last_exit(stdout, &unit);
        }
        Ok(report) => {
            let status = report
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            evidence.push_note(format!("last exit unreadable: {status}"));
        }
        Err(exc) => evidence.push_note(format!("last exit unreadable: {exc}")),
    }
    // The stderr tail comes from the same logs path `service logs` uses,
    // narrowed to the lines a failure block can show.
    match host_channel::canonical_target(&row.service.host).await {
        Ok(target) => {
            match service::tail_logs(&target, &row.service, 2 * FAILURE_STDERR_LINES, runner).await
            {
                Ok(log) => {
                    evidence.error_origin = log.error_origin;
                    evidence.error_lines = log
                        .error_body
                        .lines()
                        .take(FAILURE_STDERR_LINES)
                        .map(str::to_string)
                        .collect();
                }
                Err(exc) => evidence.push_note(format!("stderr unreadable: {exc}")),
            }
        }
        Err(exc) => evidence.push_note(format!("stderr unreadable: {exc}")),
    }
    evidence
}

fn render_status(
    rows: &[ServiceStatus],
    json: bool,
    failures: &[FailureEvidence],
) -> Result<(), CmdError> {
    // Read once for the whole table. The record is a file on this machine and
    // the answer is the same for every row in one rendering.
    let seen = observations::load();
    if json {
        let payload: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut entry = row.to_json();
                // Carried in the machine-readable form too, because the
                // consumers of `--json` are the dashboards and gates that
                // acted on a twelve-day-old `active` without ever being able
                // to see how old it was.
                let fact = observations::service_fact(&row.service.name, &row.service.host);
                entry["observed"] = json!(observations::describe_in(&seen, &fact));
                if let Some(failure) = failures.iter().find(|failure| {
                    failure.host == row.service.host
                        && failure.unit == row.service.unit_id().to_string()
                }) {
                    entry["failure"] = failure.to_json();
                }
                entry
            })
            .collect();
        return print_json(&Value::Array(payload));
    }
    // The domain column appears only when at least one row is a system
    // LaunchDaemon: that is the one domain the approved channel cannot
    // bootstrap, and a fleet of user-domain units should not pay for a
    // column that says nothing.
    let show_domain = rows
        .iter()
        .any(|row| UnitDomain::from_path(&row.service.path).requires_privileged_bootstrap());
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let fact = observations::service_fact(&row.service.name, &row.service.host);
            let mut cells = vec![
                row.service.host.clone(),
                row.service.name.clone(),
                row.service.unit_id().to_string(),
                row.service.source.clone(),
                row.state.clone(),
                dash(&row.reported_at),
                observations::describe_in(&seen, &fact),
                dash(&row.detail),
            ];
            if show_domain {
                cells.insert(3, dash(UnitDomain::from_path(&row.service.path).as_str()));
            }
            cells
        })
        .collect();
    let mut headers = vec![
        "HOST",
        "SERVICE",
        "UNIT",
        "SOURCE",
        "STATE",
        "REPORTED_AT",
        "OBSERVED",
        "DETAIL",
    ];
    if show_domain {
        headers.insert(3, "DOMAIN");
    }
    table::print(&headers, &cells);
    // A unit declared in a domain its host cannot have, named where the
    // operator is already reading the table it is missing from. This is a
    // fact about the document, so it prints for every row that carries it
    // whatever the beacon said — including the `missing` rows, where the
    // declaration is the reason the beacon reports nothing.
    for row in rows {
        if let Some(misdeclared) = &row.misdeclared_domain {
            println!("declaration: {}", misdeclared.sentence());
        }
    }
    for failure in failures {
        let exit = match &failure.last_exit {
            Some(exit) => format!("last launchd exit {exit}"),
            None => "last launchd exit unknown".to_string(),
        };
        println!("failure: {} {}: {}", failure.host, failure.unit, exit);
        // A failed system LaunchDaemon has exactly one repair over the
        // approved channel, and it has conditions; say which, here, where the
        // operator is reading why.
        if rows.iter().any(|row| {
            row.service.host == failure.host
                && row.service.unit_id() == failure.unit.as_str()
                && UnitDomain::from_path(&row.service.path).requires_privileged_bootstrap()
        }) {
            println!(
                "  unit: system LaunchDaemon — `service restart` can only end its process and let \
                 launchd's KeepAlive replace it; loading it takes sudo on the host"
            );
        }
        if let Some(error_origin) = &failure.error_origin {
            println!("  stderr: {error_origin}");
        }
        for line in &failure.error_lines {
            println!("  {line}");
        }
        if let Some(note) = &failure.note {
            println!("  note: {note}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Restart
// ---------------------------------------------------------------------------
async fn host_sudo_password(target: &crate::targets::ComputeTarget) -> Result<Option<String>, CmdError> {
    let Some(item) = target.account_ref.as_deref() else {
        return Ok(None);
    };
    crate::credential_store::read_string(item, "password")
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "cannot read {item}#password for privileged lifecycle on {}: {error}",
                target.name
            ))
        })
        .map(|password| password.filter(|value| !value.is_empty()))
}


async fn restart(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let sudo_password = if UnitDomain::from_path(&declared.path).requires_privileged_bootstrap()
        {
            host_sudo_password(&target).await?
        } else {
            None
        };
        let report = service::restart_service_with_password(
            &target,
            declared,
            sudo_password.as_deref(),
            &runner,
        )
        .await
        .map_err(click)?;
        if !report.succeeded("restarted") {
            failures.push(format!("{}: {}", declared.host, report.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            // The domain, in the human output as well as in `--json`: a
            // restart that acted in `user/501` and a restart that acted in
            // `gui/501` are different operations, and the table used to print
            // neither.
            dash(&report.domain),
            dash(&report.status),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "DOMAIN", "STATUS", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "restart")
}

async fn update(
    name: &str,
    host: &str,
    reference: Option<&str>,
    archive: Option<&str>,
    rollback_to: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    // The service must already be managed here: this moves a unit forward, and
    // silently installing a version for a unit nobody runs would look like a
    // deployment while changing nothing.
    let services = declared_matching(name, Some(host)).await?;
    if services.is_empty() {
        return Err(CmdError::click(format!(
            "{host} does not manage {name}; deploy it first"
        )));
    }
    // The registry name is the unit label; the artifact directory is whatever
    // the unit's own program path reads from. Deriving it from the unit means
    // the new version lands where the running one is actually read, instead of
    // beside it under a directory that only matches the name.
    let runner = production_runner();
    let declared = &services[usize::default()];
    let report = service::show_service(&target, declared, &runner)
        .await
        .map_err(click)?;
    let program = report.detail.trim();
    let directory = program
        .split("/services/")
        .nth(usize::from(true))
        .and_then(|rest| rest.split('/').next())
        .filter(|segment| !segment.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{host}: {name} runs {program:?}, which is not under a managed services directory,                  so there is no artifact directory to update"
            ))
        })?;
    // Two sources, one install. A published artifact is the durable route; a
    // local archive is how a bundle reaches a host before there is an object
    // store the whole fleet can read, and it is checksummed on the far side the
    // same way rather than trusted for arriving.
    if let Some(version) = rollback_to {
        let script = format!(
            "set -euo pipefail\nname={}\nversion={}\n{ROLLBACK_BODY}",
            crate::deploy::shlex_quote(directory),
            crate::deploy::shlex_quote(version),
        );
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(click)?;
        if !output.ok() {
            return Err(CmdError::click(format!(
                "{host}: {}",
                host_channel::last_error_line(&output, "rollback failed")
            )));
        }
        println!("{host}: {name} -> {version} (takes effect on the next restart)");
        return Ok(());
    }
    let installed = match (reference, archive) {
        (Some(reference), None) => install_from_artifact(&target, directory, reference).await?,
        (None, Some(path)) => install_from_archive(&target, directory, path, &runner).await?,
        (None, None) => {
            return Err(CmdError::click(
                "update needs --from-artifact REF or --from-archive PATH",
            ))
        }
        (Some(_), Some(_)) => {
            return Err(CmdError::click(
                "--from-artifact and --from-archive are exclusive",
            ))
        }
    };
    // A unit pinned to a version directory never sees an install: `current`
    // moves and the job keeps executing the path it was rendered with, so the
    // deployment reports success and the machine runs what it ran before. Point
    // it at `current`, which is what makes a later install or a rollback a
    // relink rather than a redeploy.
    let followed = follow_current(&target, declared, directory, &runner).await?;
    if json {
        print_json(&json!({
            "host": host,
            "service": name,
            "unit_repointed": followed,
            "version": installed.version,
            "sha256": installed.sha256,
            "status": "updated",
            "effective": "on next restart",
        }))?;
    } else {
        println!(
            "{host}: {name} -> {} (takes effect on the next restart)",
            installed.version
        );
    }
    Ok(())
}

async fn show(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let report = service::show_service(&target, declared, &runner)
            .await
            .map_err(click)?;
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "RUNS"], &cells);
    }
    Ok(())
}

async fn stop(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let report = service::stop_service(&target, declared, &runner)
            .await
            .map_err(click)?;
        if !report.succeeded("stopped") {
            failures.push(format!("{}: {}", declared.host, report.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&report.domain),
            dash(&report.status),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "DOMAIN", "STATUS", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "stop")
}

pub(crate) async fn service_secret(item: &str, field: &str) -> Result<String, CmdError> {
    let vault = crate::skarbiec::Client::service_verifier()
        .map_err(|err| CmdError::click(err.to_string()))?;
    // Both callers -- auth-check and secret-sync -- want exactly one field, and
    // asking for the whole item is refused outright by a broker that requires a
    // named field. Ask for what is wanted.
    let stored = vault
        .read_field(item, field)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    stored
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::click(format!(
                "Skarbiec item {item:?} has no non-empty string field {field:?}"
            ))
        })
}

struct SecretSyncOptions<'a> {
    name: &'a str,
    host: &'a str,
    item: &'a str,
    field: &'a str,
    variable: &'a str,
    env_file: &'a str,
    restart_after_sync: bool,
    as_json: bool,
}

async fn secret_sync(options: SecretSyncOptions<'_>) -> Result<(), CmdError> {
    let SecretSyncOptions {
        name,
        host,
        item,
        field,
        variable,
        env_file,
        restart_after_sync,
        as_json,
    } = options;
    let secret = service_secret(item, field).await?;

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let synced =
            service::sync_service_secret(&target, declared, env_file, variable, &secret, &runner)
                .await
                .map_err(click)?;
        let sync_ok = synced.succeeded("secret_synced");
        if !sync_ok {
            failures.push(format!("{}: {}", declared.host, synced.failure()));
        }

        let restarted = if sync_ok && restart_after_sync {
            Some(
                service::restart_service(&target, declared, &runner)
                    .await
                    .map_err(click)?,
            )
        } else {
            None
        };
        if let Some(report) = &restarted {
            if !report.succeeded("restarted") {
                failures.push(format!("{}: {}", declared.host, report.failure()));
            }
        }

        let restart_status = match &restarted {
            Some(report) => dash(&report.status),
            None if restart_after_sync => "skipped".to_string(),
            None => "-".to_string(),
        };
        let detail = restarted
            .as_ref()
            .map(|report| report.detail.as_str())
            .filter(|detail| !detail.is_empty())
            .unwrap_or(&synced.detail);
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&synced.status),
            restart_status,
            dash(detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "item": item,
            "field": field,
            "variable": variable,
            "env_file": env_file,
            "sync": synced.to_json(),
            "restart": restarted.as_ref().map(|report| report.to_json()),
        }));
    }
    drop(secret);

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "SYNC", "RESTART", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "secret sync")
}

struct AuthCheckOptions<'a> {
    name: &'a str,
    host: &'a str,
    item: &'a str,
    field: &'a str,
    url: &'a str,
    post_empty_json: bool,
    expect_status: Option<u16>,
    repair: bool,
    take_over_listener: bool,
    variable: Option<&'a str>,
    env_file: Option<&'a str>,
    as_json: bool,
}

async fn auth_check(options: AuthCheckOptions<'_>) -> Result<(), CmdError> {
    let AuthCheckOptions {
        name,
        host,
        item,
        field,
        url,
        post_empty_json,
        expect_status,
        repair,
        take_over_listener,
        variable,
        env_file,
        as_json,
    } = options;
    let repair_target = if repair {
        Some((
            variable.ok_or_else(|| CmdError::click("--repair requires --variable"))?,
            env_file.ok_or_else(|| CmdError::click("--repair requires --env-file"))?,
        ))
    } else {
        None
    };
    let secret = service_secret(item, field).await?;
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let initial = service::check_service_bearer(
            &target,
            declared,
            url,
            &secret,
            post_empty_json,
            expect_status,
            &runner,
        )
        .await
        .map_err(click)?;
        let mut final_report = initial.clone();
        let mut synced = None;
        let mut restarted = None;
        let mut listener_reset = None;

        if !initial.succeeded("auth_ok") {
            if let Some((variable, env_file)) = repair_target {
                let sync_report = service::sync_service_secret(
                    &target, declared, env_file, variable, &secret, &runner,
                )
                .await
                .map_err(click)?;
                let sync_ok = sync_report.succeeded("secret_synced");
                synced = Some(sync_report);
                if sync_ok {
                    let restart_report = service::restart_service(&target, declared, &runner)
                        .await
                        .map_err(click)?;
                    let restart_ok = restart_report.succeeded("restarted");
                    restarted = Some(restart_report);
                    if restart_ok {
                        final_report = service::check_service_bearer(
                            &target,
                            declared,
                            url,
                            &secret,
                            post_empty_json,
                            expect_status,
                            &runner,
                        )
                        .await
                        .map_err(click)?;
                    }
                }
            }
        }

        if repair
            && take_over_listener
            && !final_report.succeeded("auth_ok")
            && synced
                .as_ref()
                .is_some_and(|report| report.succeeded("secret_synced"))
        {
            let reset_report = service::reset_service_listener(&target, declared, url, &runner)
                .await
                .map_err(click)?;
            let reset_ok = reset_report.succeeded("listener_stopped")
                || reset_report.succeeded("listener_absent");
            listener_reset = Some(reset_report);
            if reset_ok {
                let restart_report = service::restart_service(&target, declared, &runner)
                    .await
                    .map_err(click)?;
                let restart_ok = restart_report.succeeded("restarted");
                restarted = Some(restart_report);
                if restart_ok {
                    final_report = service::check_service_bearer(
                        &target,
                        declared,
                        url,
                        &secret,
                        post_empty_json,
                        expect_status,
                        &runner,
                    )
                    .await
                    .map_err(click)?;
                }
            }
        }

        let ok = final_report.succeeded("auth_ok");
        if !ok {
            failures.push(format!("{}: {}", declared.host, final_report.failure()));
        }
        let repair_status = listener_reset
            .as_ref()
            .or(synced.as_ref())
            .map(|report| report.status.as_str())
            .unwrap_or("-");
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&final_report.status),
            dash(repair_status),
            dash(&final_report.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "item": item,
            "field": field,
            "url": url,
            "post_empty_json": post_empty_json,
            "expect_status": expect_status,
            "initial": initial.to_json(),
            "sync": synced.as_ref().map(|report| report.to_json()),
            "restart": restarted.as_ref().map(|report| report.to_json()),
            "listener_reset": listener_reset.as_ref().map(|report| report.to_json()),
            "final": final_report.to_json(),
            "ok": ok,
        }));
    }
    drop(secret);

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "AUTH", "REPAIR", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "authentication check")
}

/// Report a partial failure after the per-host results have been printed,
/// so the operator sees which hosts worked as well as which did not.
fn fail_if_any(failures: &[String], action: &str) -> Result<(), CmdError> {
    if failures.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{action} failed on {}",
        failures.join("; ")
    )))
}

// ---------------------------------------------------------------------------
// Adopt / retire / deploy — the registry mutations
// ---------------------------------------------------------------------------

/// Declare a service through the validated write path.
///
/// `push_document` runs `targets::validate_registry` before it writes, so a
/// declaration that would produce an invalid registry is refused with
/// Nothing uploaded. Returns the new generation.
async fn record_declaration(record: &ManagedService) -> Result<String, CmdError> {
    let mut document = registry::fetch_document().await?;
    // A placeholder left by `service declare` is the declaration, not the
    // unit: deploy replaces it with the real record rather than refusing on
    // a name it put there itself.
    if let Some(services) = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|targets| {
            targets
                .iter_mut()
                .find(|target| {
                    target.get("name").and_then(Value::as_str) == Some(record.host.as_str())
                })
                .and_then(|target| target.get_mut("services"))
                .and_then(Value::as_array_mut)
        })
    {
        services.retain(|existing| {
            !(existing.get("name").and_then(Value::as_str) == Some(record.name.as_str())
                && existing.get("declared_only").and_then(Value::as_bool) == Some(true))
        });
    }
    service::add_service(&mut document, record).map_err(click)?;
    registry::push_document(&document).await
}

struct OnboardingOptions<'a> {
    name: &'a str,
    host: &'a str,
    product_id: &'a str,
    display_name: &'a str,
    repository: &'a str,
    surfaces: Vec<String>,
    first_success_fact: &'a str,
    onboarding_kind: &'a str,
    status: &'a str,
    as_json: bool,
}

async fn onboarding(options: OnboardingOptions<'_>) -> Result<(), CmdError> {
    let mut document = registry::fetch_document().await?;
    let record = service::set_service_onboarding(
        &mut document,
        options.host,
        options.name,
        service::OnboardingProduct {
            product_id: options.product_id.to_string(),
            display_name: options.display_name.to_string(),
            repository: options.repository.to_string(),
            surface_kinds: options.surfaces,
            first_success_fact: options.first_success_fact.to_string(),
            onboarding_kind: options.onboarding_kind.to_string(),
            status: options.status.to_string(),
        },
    )
    .map_err(click)?;
    let generation = registry::push_document(&document).await?;
    render_mutation("onboarding", &record, &generation, None, options.as_json)
}

async fn adopt(
    unit: &str,
    host: Option<&str>,
    host_heuristic: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let (target, host_heuristic) = resolve_placement(host, host_heuristic).await?;
    let host = target.name.clone();
    let runner = production_runner();
    let report = service::probe_service(&target, unit, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("probed") {
        return Err(CmdError::click(format!(
            "{host}: could not probe {unit}: {}",
            report.failure()
        )));
    }
    // Adoption claims a unit that is already there. Declaring one that is
    // not present is how a registry starts describing a fleet that does not
    // exist, which is the failure this command was written against.
    if report.file_state != "present" && report.unit_state != "loaded" {
        return Err(CmdError::click(format!(
            "{unit} is not present on {host}: no unit file at {} and the init system does not know it",
            report.path
        )));
    }

    let record =
        service::record_from_report(&host, host_heuristic.as_deref(), unit, &report, &now());
    let generation = record_declaration(&record).await?;
    render_mutation(
        "adopted",
        &record,
        &generation,
        Some(&report.to_json()),
        json,
    )
}

async fn retire(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let declared = service::declared_services(&target);
    let Some(found) = declared.iter().find(|candidate| candidate.matches(unit)) else {
        return Err(unmanaged(unit, Some(host)));
    };
    if found.source == SOURCE_RECOVERY {
        return Err(CmdError::click(format!(
            "{unit} is carried by the fixed host-recovery program, not by the registry entry \
             for {host}; it cannot be retired. Adopt it first if you need it under registry \
             management."
        )));
    }

    let runner = production_runner();
    let report = service::retire_service(&target, found, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("retired") {
        // Forgetting a unit that is still running is exactly the state this
        // command family exists to prevent, so the declaration stays until
        // the host confirms it is stopped.
        return Err(CmdError::click(format!(
            "{host}: could not stop {unit}: {}; it is still declared in the registry",
            report.failure()
        )));
    }

    let mut document = registry::fetch_document().await?;
    let removed = service::remove_service(&mut document, host, unit).map_err(click)?;
    let generation = registry::push_document(&document).await?;
    render_mutation(
        "retired",
        &removed,
        &generation,
        Some(&report.to_json()),
        json,
    )
}

/// `service remove`: the whole of "remove this service", composed from the
/// three halves the product already owns — stop and forget on the host
/// (`retire`), drop the registry entry, and delete the declared unit file
/// (`host remove-file`). The file path is the declaration's, which is the
/// only path worth trusting here: an operator-typed path would make this a
/// delete-anything verb, and a wrong delete on someone else's machine is the
/// failure every guard in `remove-file` exists against.
///
/// Partial states are said, not hidden: a stopped-and-forgotten service
/// whose file the channel may not delete is `retired` with the file named
/// and the privileged command beside it, and the command exits non-zero
/// because the asked-for end state did not happen.
async fn remove(unit: &str, host: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let declared = service::declared_services(&target);
    let Some(found) = declared.iter().find(|candidate| candidate.matches(unit)) else {
        return Err(unmanaged(unit, Some(host)));
    };
    if found.source == SOURCE_RECOVERY {
        return Err(CmdError::click(format!(
            "{unit} is carried by the fixed host-recovery program, not by the registry entry \
             for {host}; it cannot be removed. Adopt it first if you need it under registry \
             management."
        )));
    }
    let path = found.path.clone();

    let runner = production_runner();
    let report = service::retire_service(&target, found, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("retired") {
        return Err(CmdError::click(format!(
            "{host}: could not stop {unit}: {}; it is still declared in the registry, and its file was not touched",
            report.failure()
        )));
    }

    let mut document = registry::fetch_document().await?;
    let removed = service::remove_service(&mut document, host, unit).map_err(click)?;
    let generation = registry::push_document(&document).await?;

    // The registry is already clean: the file half runs last, because a
    // failed delete must leave a service the fleet can still see, not a file
    // nobody declared. Its report is the second document of the answer.
    let file = crate::cli::host::remove_file_document(&target.name, &path).await;
    if json {
        let (file_status, file_detail) = match &file {
            Ok(outcome) => (outcome.status.clone(), outcome.detail.clone()),
            Err(error) => ("failed".to_string(), Some(error.to_string())),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target.name,
                "unit": unit,
                "action": "removed",
                "generation": generation,
                "file": {
                    "path": path,
                    "status": file_status,
                    "detail": file_detail,
                },
                "report": report.to_json(),
            }))?
        );
    } else {
        render_mutation(
            "removed",
            &removed,
            &generation,
            Some(&report.to_json()),
            json,
        )?;
        match &file {
            Ok(outcome) if outcome.succeeded() => {
                println!("{}: {} {}", outcome.target, outcome.path, outcome.status)
            }
            Ok(outcome) => println!("{}: {} {}", outcome.target, outcome.path, outcome.status),
            Err(error) => println!("{error}"),
        }
    }
    file.map(|_| ())
}

/// One declared service name: lowercase letters and digits at the edges,
/// with '.', '-' and '_' inside — the same rule the directory validator
/// applies, enforced here so a bad name fails before the document moves.
fn declaration_name_ok(value: &str) -> bool {
    let edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && edge(bytes[0])
        && edge(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| edge(*byte) || matches!(byte, b'.' | b'_' | b'-'))
}

/// Write one user-authored declaration into the service directory.
///
/// The declaration file is the whole contract: `name` and `host`, the
/// immutable `source`, the opaque `run` spec, and optionally `verify`,
/// `consumers`, and `endpoints` (or `port` as the loopback shorthand).
/// The fleet learns nothing about the service's kind from it — that is the
/// design, not a gap: anything the service knows about itself lives inside
/// the artifact and the run spec.
async fn declare(file: &str, as_json: bool) -> Result<(), CmdError> {
    let text = std::fs::read_to_string(file)
        .map_err(|error| CmdError::click(format!("{file}: {error}")))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| CmdError::click(format!("{file}: not a JSON object: {error}")))?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::usage(format!("{file}: 'name' is required")))?;
    if !declaration_name_ok(name) {
        return Err(CmdError::usage(format!(
            "{file}: 'name' must be a lowercase identifier without empty edges"
        )));
    }
    let host = value
        .get("host")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::usage(format!("{file}: 'host' is required")))?;
    let mut declaration_value = match value.get("declaration") {
        Some(declaration) => declaration.clone(),
        None => {
            let mut assembled = json!({"source": value.get("source")});
            if let Some(run) = value.get("run") {
                assembled["run"] = run.clone();
            }
            assembled
        }
    };
    let declaration: crate::declaration::ServiceDeclaration =
        serde_json::from_value(std::mem::take(&mut declaration_value)).map_err(|error| {
            CmdError::usage(format!(
                "{file}: declaration must carry source.artifact and source.sha256: {error}"
            ))
        })?;
    let location = format!("service_directory.services.{name}");
    let problems = crate::declaration::validate(&location, &declaration);
    if !problems.is_empty() {
        return Err(CmdError::usage(problems.join("; ")));
    }
    let verify = match value.get("verify") {
        Some(descriptor) => {
            let descriptor: targets::VerifyDescriptor = serde_json::from_value(descriptor.clone())
                .map_err(|error| CmdError::usage(format!("{file}: 'verify': {error}")))?;
            let problems = targets::validate_verification(&location, &descriptor);
            if !problems.is_empty() {
                return Err(CmdError::usage(problems.join("; ")));
            }
            Some(
                serde_json::to_value(&descriptor)
                    .map_err(|error| CmdError::click(format!("verify: {error}")))?,
            )
        }
        None => None,
    };
    // Endpoints: explicit map wins; `port` is the loopback shorthand for the
    // declared host. The directory contract requires at least the active
    // host's endpoint, so neither present is an author error, not a default.
    let mut endpoints = value
        .get("endpoints")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let (Some(port), false) = (
        value.get("port").and_then(Value::as_u64),
        endpoints.contains_key(host),
    ) {
        if port == 0 || port > u16::MAX as u64 {
            return Err(CmdError::usage(format!("{file}: 'port' out of range")));
        }
        endpoints.insert(
            host.to_string(),
            json!({"url": format!("http://127.0.0.1:{port}")}),
        );
    }
    if !endpoints.contains_key(host) {
        return Err(CmdError::usage(format!(
            "{file}: the declaration needs an endpoint for {host} — pass 'endpoints' or the 'port' shorthand"
        )));
    }
    // The directory answers "who may call it" from the same entry, so a
    // declaration without consumers is not a declaration the contract can
    // publish: it would answer that question with silence.
    match value.get("consumers").and_then(Value::as_object) {
        Some(consumers) if !consumers.is_empty() => {}
        _ => {
            return Err(CmdError::usage(format!(
                "{file}: 'consumers' is required and must name at least one caller"
            )));
        }
    }

    let mut document = registry::fetch_document().await?;
    let known_target = document
        .get("targets")
        .and_then(Value::as_array)
        .is_some_and(|targets| {
            targets
                .iter()
                .any(|target| target.get("name").and_then(Value::as_str) == Some(host))
        });
    if !known_target {
        return Err(CmdError::usage(format!(
            "{file}: 'host' names {host}, which is not a registry target"
        )));
    }
    let directory = document
        .get_mut("service_directory")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click(
                "registry has no service_directory; an authority must publish it before services can be declared",
            )
        })?;
    let services = directory
        .entry("services")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| CmdError::click("service_directory.services: must be an object"))?;
    let entry = services
        .entry(name.to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            CmdError::click(format!(
                "service_directory.services.{name}: must be an object"
            ))
        })?;
    entry.insert("active_host".to_string(), json!(host));
    entry.insert("endpoints".to_string(), Value::Object(endpoints));
    // The directory contract binds every fixed route to the managed unit on
    // its active host, and the unit lands when `deploy` runs — so declare
    // writes the link now and a placeholder record on the host, which
    // `deploy` replaces with the real one. Declared-but-not-yet-deployed is
    // a designed state, not an error: the registry says what should run,
    // the beacons say what does.
    entry.insert("managed_service".to_string(), json!(name));
    if let Some(consumers) = value.get("consumers") {
        entry.insert("consumers".to_string(), consumers.clone());
    }
    if let Some(descriptor) = verify {
        entry.insert("verify".to_string(), descriptor);
    }
    entry.insert(
        "declaration".to_string(),
        serde_json::to_value(&declaration)
            .map_err(|error| CmdError::click(format!("declaration: {error}")))?,
    );
    let target_entry = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|targets| {
            targets
                .iter_mut()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
        })
        .ok_or_else(|| CmdError::click(format!("registry targets lost {host}")))?;
    let host_services = target_entry
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target: must be an object"))?
        .entry("services")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| {
            CmdError::click(format!("registry target {host}: services must be an array"))
        })?;
    let already = host_services
        .iter()
        .any(|record| record.get("name").and_then(Value::as_str) == Some(name));
    if !already {
        host_services.push(json!({"name": name, "declared_only": true}));
    }
    let generation = registry::push_document(&document).await?;
    if !as_json {
        println!("declared {name} on {host} (generation {generation})");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "declared": name,
                "host": host,
                "generation": generation,
            }))
            .map_err(|error| CmdError::click(format!("declare report: {error}")))?
        );
    }
    Ok(())
}

async fn deploy(
    name: &str,
    host: Option<&str>,
    host_heuristic: Option<&str>,
    from: Option<String>,
    from_artifact: Option<String>,
    args: &[String],
    json: bool,
) -> Result<(), CmdError> {
    let (target, host_heuristic) = resolve_placement(host, host_heuristic).await?;
    let host = target.name.clone();
    // Exactly one source. Neither is a sensible default: a path deploys
    // whatever is on the host with no version identity, and an artifact
    // deploys a named version; guessing between them is how a host ends up
    // running something nobody can name.
    let mut declaration_args: Vec<String> = Vec::new();
    let (from, installed) = match (from, from_artifact) {
        (Some(path), None) => (path, None),
        (None, Some(reference)) => {
            let installed = install_from_artifact(&target, name, &reference).await?;
            (installed.program_path.clone(), Some(installed))
        }
        (None, None) => {
            // A declared service deploys from its declaration: `service
            // declare` already wrote the artifact reference and the run
            // spec, so the name alone is enough.
            let document = registry::fetch_document().await?;
            let entry = document
                .get("service_directory")
                .and_then(|directory| directory.get("services"))
                .and_then(|services| services.get(name))
                .cloned();
            let Some(entry) = entry else {
                return Err(CmdError::click(format!(
                    "deploy needs --from PATH or --from-artifact REF, or a declaration written by \
                     `stado service declare --file`; the directory names no service '{name}'"
                )));
            };
            let Some(declared) = crate::declaration::ServiceDeclaration::from_entry(&entry) else {
                return Err(CmdError::click(format!(
                    "deploy needs --from PATH or --from-artifact REF: '{name}' is declared without a source"
                )));
            };
            let installed = install_from_artifact(&target, name, &declared.source.artifact).await?;
            if installed.sha256 != declared.source.sha256 {
                return Err(CmdError::click(format!(
                    "{name}: declaration pins sha256 {} but the artifact installed {}",
                    declared.source.sha256, installed.sha256
                )));
            }
            if args.is_empty() {
                declaration_args = declared.run.args;
            }
            (installed.program_path.clone(), Some(installed))
        }
        (Some(_), Some(_)) => {
            return Err(CmdError::click("--from and --from-artifact are exclusive"))
        }
    };
    let from = from.as_str();
    let args: &[String] = if args.is_empty() {
        &declaration_args
    } else {
        args
    };
    let plan = service::plan_deploy(name, from, args).map_err(click)?;

    // Refuse a colliding declaration BEFORE touching the host: pushing a
    // unit that then cannot be recorded would leave an unmanaged unit
    // running, which is the whole failure this command family closes.
    let declared = service::declared_services(&target);
    for taken in [name, plan.label.as_str(), plan.unit.as_str()] {
        if declared.iter().any(|candidate| candidate.matches(taken)) {
            return Err(CmdError::click(format!(
                "{host} already manages {taken}; retire it first"
            )));
        }
    }

    let runner = production_runner();
    let report = service::deploy_service(&target, &plan, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("deployed") {
        return Err(CmdError::click(format!(
            "{host}: could not deploy {name}: {}",
            report.failure()
        )));
    }

    let record =
        service::record_from_report(&host, host_heuristic.as_deref(), name, &report, &now());
    let generation = match record_declaration(&record).await {
        Ok(generation) => generation,
        // The unit is on the host and running; only the declaration failed.
        // Reporting that as a bare registry error would leave exactly the
        // running-but-unmanaged state this command family closes, so say
        // what happened and name the one command that repairs it.
        Err(exc) => {
            let detail = exc
                .message
                .unwrap_or_else(|| "registry write failed".to_string());
            return Err(CmdError::click(format!(
                "{host}: {name} is deployed and running, but recording it failed: {detail}. \
                 Run `stado service adopt {} --host {host}` to bring it under management.",
                record.unit_id()
            )));
        }
    };
    // The version is the point of --from-artifact: without it the operator is
    // back to "something is deployed" with no way to say what. Reported beside
    // the unit rather than inside the remote report, which describes the host
    // action and not what was installed.
    if let Some(installed) = installed.as_ref() {
        if !json {
            println!(
                "installed {name} version {} (sha256 {})",
                installed.version, installed.sha256
            );
        }
    }
    render_mutation(
        "deployed",
        &record,
        &generation,
        Some(&report.to_json()),
        json,
    )
}

struct EnsureOptions<'a> {
    name: &'a str,
    host: &'a str,
    from: Option<&'a str>,
    args: &'a [String],
    reason: &'a str,
    as_json: bool,
}

/// What a unit runs, and which declaration said so.
struct UnitProgram {
    program: String,
    args: Vec<String>,
    /// `"flag"`, `"registry"` or `"shipped"`.
    source: &'static str,
    /// Stable unit identity supplied by the catalog. The operator-facing
    /// product name and launchd label are not required to be the same.
    unit: Option<String>,
}

/// The launchd label a declaration carries, or a systemd unit name without
/// its suffix — the spelling `deploy::service::plan_deploy_labelled` renders
/// at.
fn declared_label(service: &ManagedService) -> Option<&str> {
    let unit_id = service.unit_id();
    Some(unit_id.strip_suffix(".service").unwrap_or(unit_id)).filter(|label| !label.is_empty())
}

/// The program and argument vector `ensure` renders the unit from.
///
/// Flags win, because an operator correcting a wrong declaration has to be
/// able to. Absent them, the host's own `services[]` entry answers: a
/// declaration that carries its program is one this command can reinstall
/// from the document alone, which is the whole difference between a declared
/// service and a plist somebody installed by hand — the resolver and the
/// local dashboard on `lukasz-macbook` were the second kind, and nothing in
/// the product knew their restart policy. Last, the declaration shipped in
/// this build ([`targets::load_bundled_registry`]), which is how a unit
/// declared in a release reaches a canonical document published before it:
/// the first `ensure` writes it there.
fn unit_program(
    host: &str,
    name: &str,
    from: Option<&str>,
    args: &[String],
    declared: Option<&ManagedService>,
) -> Result<UnitProgram, CmdError> {
    if let Some(from) = from {
        return Ok(UnitProgram {
            program: from.to_string(),
            args: args.to_vec(),
            source: "flag",
            unit: None,
        });
    }
    if !args.is_empty() {
        return Err(CmdError::usage(
            "--arg needs --from: an argument vector without the program it belongs to would be \
             appended to a declared program the caller never named",
        ));
    }
    if let Some(declared) = declared.filter(|candidate| !candidate.program.is_empty()) {
        return Ok(UnitProgram {
            program: declared.program.clone(),
            args: declared.args.clone(),
            source: "registry",
            unit: Some(declared.unit_id().to_string()),
        });
    }
    // The shipped Wisent catalog answers by name, on any host, with no
    // declaration of the operator's own — that is the whole of "run Weles
    // here" as one word.
    if let Some(entry) = crate::deploy::service_catalog::lookup(name)
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        if !entry.available {
            return Err(CmdError::click(format!(
                "{name} is a Wisent product service, but it is not installable from Stado yet: {}",
                entry
                    .unavailable_reason
                    .as_deref()
                    .unwrap_or("the catalog names no host-service install contract")
            )));
        }
        return Ok(UnitProgram {
            // Placeholders survive here on purpose: `$HOME` and
            // `$STADO_PLATFORM` belong to the target, and only the caller
            // holding the resolved target may expand them.
            program: entry.program,
            args: entry.args,
            source: "catalog",
            unit: entry.unit,
        });
    }
    let bundled =
        targets::load_bundled_registry().map_err(|error| CmdError::click(error.to_string()))?;
    let shipped = bundled
        .lookup(host)
        .map(service::declared_services)
        .unwrap_or_default()
        .into_iter()
        .find(|candidate| candidate.matches(name) && !candidate.program.is_empty());
    if let Some(shipped) = shipped {
        let unit = shipped.unit_id().to_string();
        return Ok(UnitProgram {
            program: shipped.program,
            args: shipped.args,
            source: "shipped",
            unit: Some(unit),
        });
    }
    Err(CmdError::usage(format!(
        "nothing declares what {name} runs on {host}: pass --from PATH (repeating --arg for each \
         argument), give its registry services[] entry a \"program\" and \"args\", or pick one of \
         the preconfigured Wisent services `stado service catalog` lists"
    )))
}

/// `service catalog`: the preconfigured Wisent services this build ships,
/// printed as they would deploy. Read-only and local: the answer comes from
/// the compiled-in document, never from a host.
async fn catalog(json: bool) -> Result<(), CmdError> {
    let entries = crate::deploy::service_catalog::all()
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "services": entries.iter().map(|entry| json!({
                    "name": entry.name,
                    "summary": entry.summary,
                    "program": entry.program,
                    "args": entry.args,
                    "unit": entry.unit,
                    "available": entry.available,
                    "unavailable_reason": entry.unavailable_reason,
                })).collect::<Vec<_>>(),
            }))?
        );
    } else {
        for entry in &entries {
            if entry.available {
                println!(
                    "{:<24} {} {}",
                    entry.name,
                    entry.program,
                    entry.args.join(" ")
                );
            } else {
                println!("{:<24} unavailable", entry.name);
            }
            println!("{:<24} {}", "", entry.summary);
            if let Some(reason) = &entry.unavailable_reason {
                println!("{:<24} {}", "", reason);
            }
        }
    }
    Ok(())
}

/// `service ensure NAME --host HOST [--from PATH] --reason WHY`.
///
/// The idempotent half of `deploy`, and the only one that works on an ssh
/// login with no Aqua session. Two facts decide everything it does, and both
/// come from the host: what the unit on the box declares it runs, and what the
/// process under it is actually running. See
/// [`crate::deploy::service::ensure_service`].
async fn ensure(options: EnsureOptions<'_>) -> Result<(), CmdError> {
    let reason = options.reason.trim();
    if reason.is_empty() {
        return Err(CmdError::usage(
            "--reason must say why this host has to run this unit; it is recorded beside the \
             registry document this command declares the unit in",
        ));
    }
    let target = host_channel::canonical_target(options.host)
        .await
        .map_err(click)?;
    let host = target.name.clone();

    // Resolve both the operator-facing product name and the stable catalog
    // unit. An older registry may carry only the latter; treating that as no
    // declaration minted a duplicate unit beside the canonical daemon.
    let declared = service::declared_services(&target);
    let catalog_unit = crate::deploy::service_catalog::lookup(options.name)
        .map_err(|error| CmdError::click(error.to_string()))?
        .and_then(|entry| entry.unit);
    let existing = declared.iter().find(|candidate| {
        candidate.matches(options.name)
            || catalog_unit
                .as_deref()
                .is_some_and(|unit| candidate.matches(unit))
    });
    let mut unit = unit_program(&host, options.name, options.from, options.args, existing)?;
    if unit.source == "catalog" {
        let entry = crate::deploy::service_catalog::CatalogService {
            name: options.name.to_string(),
            summary: String::new(),
            unit: unit.unit.clone(),
            program: unit.program.clone(),
            args: unit.args.clone(),
            available: true,
            unavailable_reason: None,
        };
        let (program, args) = crate::deploy::service_catalog::resolve_entry(
            &entry,
            &crate::deploy::service_catalog::home_for(&target),
            Some(&target.release_platform),
            &target.name,
        );
        unit.program = program;
        unit.args = args;
        eprintln!(
            "{host} declares no program for {}; rendering the unit from the Wisent service \
             catalog this build ships: {} {}",
            options.name,
            unit.program,
            unit.args.join(" ")
        );
    }
    if unit.source == "shipped" {
        eprintln!(
            "{host} declares no program for {}; rendering the unit from the declaration shipped \
             with this build: {} {}",
            options.name,
            unit.program,
            unit.args.join(" ")
        );
    }
    // At the label the declaration already carries, when it carries one. A
    // minted label for a unit that exists under another one is a second unit,
    // not this one.
    let plan = match unit
        .unit
        .as_deref()
        .or_else(|| existing.and_then(declared_label))
    {
        Some(label) => {
            service::plan_deploy_labelled(options.name, label, &unit.program, &unit.args)
        }
        None => service::plan_deploy(options.name, &unit.program, &unit.args),
    }
    .map_err(click)?;

    // An existing declaration is not a refusal here, and that is the whole
    // difference from `deploy`: asserting a unit that is already declared and
    // already running is what makes this safe to run twice, or from a script.
    let already = declared.into_iter().find(|candidate| {
        candidate.matches(options.name)
            || candidate.matches(&plan.label)
            || candidate.matches(&plan.unit)
    });

    let runner = production_runner();
    let outcome = service::ensure_service(&target, &plan, &runner)
        .await
        .map_err(click)?;
    if !outcome.succeeded() {
        let mut detail = format!(
            "{host}: could not ensure {}: {}",
            options.name,
            outcome.report.failure()
        );
        if !outcome.report.postcondition_held() {
            // A unit that will not stay up on a host where the same program
            // already runs outside any unit is the four-day incident from the
            // other side: launchd is being asked for a port a disowned process
            // still holds.
            detail.push_str(
                ". `stado service list --unowned` names a process that may still hold its port",
            );
        }
        return Err(CmdError::click(detail));
    }

    let mut record = service::record_from_ensure(&host, options.name, &outcome, &now());
    record.program = unit.program;
    record.args = unit.args;
    let generation = match &already {
        // Declared, at the same file and running the same program, by the
        // registry: the document already says what this pass just confirmed,
        // so nothing is written to it.
        Some(existing)
            if existing.source == SOURCE_REGISTRY
                && existing.path == record.path
                && existing.kind == record.kind
                && existing.program == record.program
                && existing.args == record.args =>
        {
            None
        }
        // Declared by the registry at a different file. The system-domain
        // daemon path is not the per-login agent path, and a declaration
        // naming a file the host does not have is one no later command can
        // act on, so the declaration is corrected in one document write.
        Some(existing) if existing.source == SOURCE_REGISTRY => {
            let mut document = registry::fetch_document().await?;
            service::remove_service(&mut document, &host, existing.unit_id()).map_err(click)?;
            service::add_service(&mut document, &record).map_err(click)?;
            Some(registry::push_document(&document).await?)
        }
        // Undeclared, or carried by the fixed host-recovery list. Both become
        // an explicit registry declaration, which for the recovery case is
        // exactly what `service adopt` is for.
        _ => Some(record_declaration(&record).await?),
    };

    // Recorded only when something actually changed. An audit trail that also
    // records the passes which changed nothing is one nobody reads.
    let audited = if outcome.changed() || generation.is_some() {
        Some(record_ensure_audit(&record, &outcome, &plan, reason, generation.as_deref()).await?)
    } else {
        None
    };

    if options.as_json {
        // Exactly the contract's keys: a desktop client consumes this shape.
        // Where the record landed goes to stderr rather than into the object.
        if let Some(audited) = audited.as_deref() {
            eprintln!("audit record {audited}");
        }
        return print_json(&json!({
            "host": host,
            "name": record.name,
            "label": record.unit_id(),
            "domain": outcome.domain_word(),
            "action": outcome.action,
            "pid": outcome.pid.trim().parse::<u32>().ok(),
        }));
    }
    table::print(
        &["HOST", "SERVICE", "LABEL", "DOMAIN", "ACTION", "PID"],
        &[vec![
            host,
            record.name.clone(),
            record.unit_id().to_string(),
            outcome.domain_word().to_string(),
            outcome.action.clone(),
            dash(outcome.pid.trim()),
        ]],
    );
    if let Some(audited) = audited.as_deref() {
        println!("audit record {audited}");
    }
    Ok(())
}

/// Where the record of one `ensure` pass lives, relative to the canonical
/// registry document.
const ENSURE_AUDIT_PREFIX: &str = "service_audit";

/// Append the record of one `ensure` pass beside the state it changed.
///
/// This command exists to be run from a script, and a change nobody typed is a
/// change nobody remembers making: the reason the operator gave, what the host
/// did about it, and the registry generation it produced are written as one
/// create-only object through [`targets::RegistryStore::write_beside`] — beside
/// the registry document, because the registry is the state that changed and on
/// a GCS deployment the queue store is a different bucket entirely.
async fn record_ensure_audit(
    record: &ManagedService,
    outcome: &service::EnsureOutcome,
    plan: &service::DeployPlan,
    reason: &str,
    generation: Option<&str>,
) -> Result<String, CmdError> {
    let store = targets::RegistryStore::open()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let now = chrono::Utc::now();
    let body = serde_json::to_string_pretty(&json!({
        "action": outcome.action,
        "host": record.host,
        "service": record.name,
        "unit": record.unit_id(),
        "kind": record.kind,
        "path": record.path,
        "domain": outcome.domain_word(),
        "pid": outcome.pid.trim().parse::<u32>().ok(),
        "program": plan.program,
        "argv": plan.argv,
        "reason": reason,
        "registry_generation": generation,
        "recorded_at": now.to_rfc3339(),
        "actor": crate::cli::autonomy_cmd::actor(),
    }))?;
    // Timestamp first so one host's records sort by when they happened, and
    // compact rather than RFC-3339 because the key is also a file name on the
    // local-file backend.
    let key = format!(
        "{ENSURE_AUDIT_PREFIX}/{}/{}-{}.json",
        record.host,
        now.format("%Y%m%dT%H%M%S%.6fZ"),
        record.unit_id()
    );
    let (path, created) = store
        .write_beside(&key, &body)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if !created {
        return Err(CmdError::click(format!(
            "{path} already exists, so this pass was not recorded; an audit record is never \
             replaced"
        )));
    }
    Ok(path)
}

/// `datetime.now(timezone.utc).isoformat()` as every other writer in the
/// crate stamps it (`queue/leases.rs::now_iso`).
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn render_mutation(
    action: &str,
    record: &ManagedService,
    generation: &str,
    remote: Option<&Value>,
    json: bool,
) -> Result<(), CmdError> {
    if json {
        let mut payload = serde_json::json!({
            "action": action,
            "service": record.to_json(),
            "registry_generation": generation,
        });
        if let Some(remote) = remote {
            payload["remote"] = remote.clone();
        }
        return print_json(&payload);
    }
    table::print(
        &[
            "ACTION",
            "HOST",
            "SERVICE",
            "UNIT",
            "KIND",
            "PATH",
            "GENERATION",
        ],
        &[vec![
            action.to_string(),
            record.host.clone(),
            record.name.clone(),
            record.unit_id().to_string(),
            record.kind.clone(),
            dash(&record.path),
            generation.to_string(),
        ]],
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

async fn logs(name: &str, host: Option<&str>, lines: usize, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut tails: Vec<ServiceLog> = Vec::new();
    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        tails.push(
            service::tail_logs(&target, declared, lines, &runner)
                .await
                .map_err(click)?,
        );
    }

    if json {
        let payload: Vec<Value> = tails.iter().map(ServiceLog::to_json).collect();
        return print_json(&Value::Array(payload));
    }
    for tail in &tails {
        // A log body is not tabular; it is the file. Head each one so a
        // multi-host tail stays attributable.
        println!("\n== {} {} ({}) ==", tail.host, tail.unit, tail.origin);
        print!("{}", tail.body);
        if !tail.body.ends_with('\n') {
            println!();
        }
        // stderr is its own file under launchd, so it is its own section;
        // the origin names the file, or the reason there was nothing to
        // show ("absent in plist", "<path> (empty)").
        if let Some(error_origin) = &tail.error_origin {
            println!(
                "== {} {} stderr ({}) ==",
                tail.host, tail.unit, error_origin
            );
            if !tail.error_body.is_empty() {
                print!("{}", tail.error_body);
                if !tail.error_body.ends_with('\n') {
                    println!();
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Env
// ---------------------------------------------------------------------------

async fn env(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut environments: Vec<ServiceEnv> = Vec::new();
    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let unit = service::fetch_unit_file(&target, declared, &runner)
            .await
            .map_err(click)?;
        environments.push(service::unit_environment(&unit).map_err(click)?);
    }

    if json {
        let payload: Vec<Value> = environments.iter().map(ServiceEnv::to_json).collect();
        return print_json(&Value::Array(payload));
    }

    let cells: Vec<Vec<String>> = environments
        .iter()
        .flat_map(|environment| {
            environment.env.iter().map(|(key, value)| {
                vec![
                    environment.host.clone(),
                    environment.unit.clone(),
                    key.clone(),
                    value.clone(),
                ]
            })
        })
        .collect();
    table::print(&["HOST", "UNIT", "VARIABLE", "VALUE"], &cells);

    for environment in &environments {
        for file in &environment.environment_files {
            // The pointer, not the contents: reporting it is how the
            // operator learns this listing is partial.
            println!(
                "{} {}: also reads EnvironmentFile={file} (not shown)",
                environment.host, environment.unit
            );
        }
    }
    Ok(())
}

/// Resolve one artifact reference and place that exact version on the host.
///
/// The alias is resolved before anything is written, so what lands on disk is
/// an immutable version and the path names it. Verification happens on the
/// host against the digest the manifest declares: a download that does not
/// match never becomes a running unit, and the previous `current` is left
/// where it was.
async fn install_from_artifact(
    target: &crate::targets::ComputeTarget,
    name: &str,
    reference: &str,
) -> Result<crate::deploy::artifact_install::InstalledArtifact, CmdError> {
    let registry = crate::artifacts::ArtifactRegistry::new()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let parsed = crate::artifacts_models::ArtifactRef::parse(reference)?;
    let manifest = registry.resolve_manifest(&parsed).await?;
    let runner = production_runner();
    crate::deploy::artifact_install::install_artifact(target, name, &manifest, &runner)
        .await
        .map_err(click)
}

/// Install a release archive that is not in an object store yet.
///
/// The published route is `--from-artifact`, and it stays the durable one. This
/// exists because a bundle has to reach a host before the fleet has a store
/// both machines can read, and the alternative people reach for in that gap is
/// copying a file by hand onto a running service. The archive is streamed over
/// the approved channel, checksummed on the far side, unpacked into a version
/// directory named for its own digest, and `current` is relinked only after the
/// digest matches.
async fn install_from_archive(
    target: &crate::targets::ComputeTarget,
    directory: &str,
    path: &str,
    runner: &crate::deploy::Runner,
) -> Result<crate::deploy::artifact_install::InstalledArtifact, CmdError> {
    let bytes = std::fs::read(path)?;
    let digest = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        hex::encode(hasher.finalize())
    };
    let version = format!("sha256-{}", &digest[..usize::from(12u8)]);
    let staged = format!(".stado/.{directory}-{version}.tar.gz");

    if crate::deploy::host_channel::target_is_this_host(target) {
        let home = std::env::var("HOME")
            .map_err(|_| CmdError::click("HOME is not set, so the staging path is unknown"))?;
        let destination = std::path::Path::new(&home).join(&staged);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(path, destination)?;
    } else {
        let ssh_target = target.ssh.clone().unwrap_or_default();
        if ssh_target.is_empty() {
            return Err(CmdError::click(format!(
                "{} declares no ssh destination",
                target.name
            )));
        }
        let prepare = host_channel::run_script(
            target,
            "set -euo pipefail\n/bin/mkdir -p \"$HOME/.stado\"\n/bin/chmod 700 \"$HOME/.stado\"\n",
            runner,
        )
        .await
        .map_err(click)?;
        if !prepare.ok() {
            return Err(CmdError::click(format!(
                "{}: cannot prepare the staging directory",
                target.name
            )));
        }
        let mut options = host_channel::ssh_options(&ssh_target);
        options.pop();
        let mut argv = vec!["scp".to_string(), "-q".to_string()];
        argv.extend(options.into_iter().skip(usize::from(true)));
        argv.push(path.to_string());
        argv.push(format!("{ssh_target}:{staged}"));
        let copy = runner(crate::deploy::CommandSpec::new(argv))
            .await
            .map_err(CmdError::click)?;
        if !copy.ok() {
            return Err(CmdError::click(format!(
                "{}: cannot deliver the archive: {}",
                target.name,
                copy.detail()
            )));
        }
    }

    let script = format!(
        "set -euo pipefail\nname={}\nversion={}\nexpected={}\nstaged={}\n{ARCHIVE_INSTALL_BODY}",
        crate::deploy::shlex_quote(directory),
        crate::deploy::shlex_quote(&version),
        crate::deploy::shlex_quote(&digest),
        crate::deploy::shlex_quote(&staged),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "the archive did not install")
        )));
    }
    Ok(crate::deploy::artifact_install::InstalledArtifact {
        program_path: format!("$HOME/.stado/services/{directory}/current"),
        version,
        sha256: digest,
    })
}

const ARCHIVE_INSTALL_BODY: &str = r#"
root="$HOME/.stado/services/$name"
version_dir="$root/$version"
archive="$HOME/$staged"
trap 'rm -f "$archive"' EXIT

[ -s "$archive" ] || { printf '%s\n' 'delivered archive is missing or empty' >&2; exit 1; }
actual="$(/usr/bin/shasum -a 256 "$archive" | /usr/bin/awk '{print $1}')"
if [ "$actual" != "$expected" ]; then
  printf '%s\n' "digest mismatch: expected $expected, delivered $actual" >&2
  exit 1
fi

rm -rf "$version_dir"
/bin/mkdir -p "$version_dir/darwin-arm"
/usr/bin/tar -xzf "$archive" -C "$version_dir/darwin-arm"

# `current` is a directory here on some hosts and a symlink on others; either
# way the previous one is kept beside the new version rather than deleted, so a
# rollback is a rename.
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.before-$version"
else
  rm -f "$root/current"
fi
/bin/ln -sfn "$version_dir" "$root/.current.new"
/bin/mv -f "$root/.current.new" "$root/current"
trap - EXIT
rm -f "$archive"
printf '%s\n' "$version_dir"
"#;

const ROLLBACK_BODY: &str = r#"
root="$HOME/.stado/services/$name"
target="$root/$version"
[ -d "$target" ] || { printf '%s\n' "no version directory $version on this host" >&2; exit 1; }
if [ -e "$root/current" ] && [ ! -L "$root/current" ]; then
  /bin/mv "$root/current" "$root/current.replaced-$(/bin/date -u +%Y%m%dT%H%M%SZ)"
else
  rm -f "$root/current"
fi
/bin/ln -sfn "$target" "$root/.current.new"
/bin/mv -f "$root/.current.new" "$root/current"
printf '%s\n' "$target"
"#;

/// Make the unit execute through `current`, if it was rendered against a
/// version directory instead.
///
/// Returns whether anything had to change.
async fn follow_current(
    target: &crate::targets::ComputeTarget,
    declared: &crate::deploy::service::ManagedService,
    directory: &str,
    runner: &crate::deploy::Runner,
) -> Result<bool, CmdError> {
    let report = service::show_service(target, declared, runner)
        .await
        .map_err(click)?;
    let program = report.detail.trim();
    let marker = format!("/services/{directory}/");
    let Some((root, rest)) = program.split_once(&marker) else {
        return Ok(false);
    };
    let Some((segment, tail)) = rest.split_once('/') else {
        return Ok(false);
    };
    if segment == "current" {
        return Ok(false);
    }
    let wanted = format!("{root}{marker}current/{tail}");
    let script = format!(
        "set -euo pipefail\nunit_path={}\nwanted={}\n{REPOINT_BODY}",
        crate::deploy::shlex_quote(&declared.path),
        crate::deploy::shlex_quote(&wanted),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: installed the version but could not point {} at current: {}",
            target.name,
            declared.unit_id(),
            host_channel::last_error_line(&output, "repoint failed")
        )));
    }
    Ok(true)
}

const REPOINT_BODY: &str = r#"
[ -f "$unit_path" ] || { printf '%s
' "no unit file at $unit_path" >&2; exit 1; }
case "$unit_path" in
  /Library/*) sudo_prefix="/usr/bin/sudo -n" ;;
  *) sudo_prefix="" ;;
esac
$sudo_prefix /usr/libexec/PlistBuddy -c "Set :ProgramArguments:0 $wanted" "$unit_path"   || $sudo_prefix /usr/libexec/PlistBuddy -c "Set :Program $wanted" "$unit_path"
printf '%s
' "$wanted"
"#;
