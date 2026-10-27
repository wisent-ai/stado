//! The steps of emptying a host that another command owns, each run as this
//! same Stado runs it from the command line: the fleet vault (`credentials
//! vault move`), the registry authority (`placement move --authority`), a
//! profile's standby (`placement standby`) and the release targets dropped
//! (`release policy remove-target`). The evacuation adds no second way.

use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::{CommandSpec, Runner};

/// This Stado run with `args` and `--json`; its receipt, or why it refused.
pub(super) async fn run_self(args: &[&str], runner: &Runner) -> Result<Value, CmdError> {
    let program = std::env::current_exe().map_err(|error| {
        CmdError::declaration(format!("this Stado's own executable cannot be named: {error}"))
    })?;
    let mut argv = vec![program.display().to_string()];
    argv.extend(args.iter().map(|arg| arg.to_string()));
    argv.push("--json".to_string());
    let shown = argv.join(" ");
    let output = runner(CommandSpec { argv, stdin: None })
        .await
        .map_err(|error| CmdError::click(format!("{shown} did not run: {error}")))?;
    if !output.succeeded() {
        return Err(CmdError::refused(format!("{shown} failed: {}", output.stderr.trim())));
    }
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        CmdError::click(format!("{shown} printed no readable report: {error}"))
    })
}

/// One line on stderr as a step of the run starts, so a run that stands for
/// minutes on a delivery or a vault rotation says which step it stands on.
pub(super) fn starting(step: String) {
    eprintln!("evacuate: {step}");
}

/// `stado credentials vault move --to TO --owner-uid UID`, with `--apply`
/// when `apply`. Read-only, the one thing `--apply` would refuse is the
/// refusal: a placement check reported refused. A destination keyring with no
/// secret half for UID is not one: the applied move makes it there with
/// Skarbiec's `owner key mint`.
pub(super) async fn vault(to: &str, owner_uid: &str, apply: bool, runner: &Runner) -> Result<Value, CmdError> {
    let mut args = vec!["credentials", "vault", "move", "--to", to, "--owner-uid", owner_uid];
    if apply {
        starting(format!("moving the fleet vault to {to}, its owner rotated to {owner_uid}"));
        args.push("--apply");
    }
    let report = run_self(&args, runner).await?;
    if let Some(refused) = report["placement_check"]["refused"].as_str() {
        return Err(CmdError::refused(format!("the vault's placement move would be refused: {refused}")));
    }
    // The next step reads the vault through this host's resolver.
    if apply {
        crate::cli::resolver::follow_authority(false)
            .await
            .map_err(|error| error.also(format!("the fleet vault itself moved to {to}; the run stopped before the next step")))?;
    }
    Ok(report)
}

/// `stado placement standby PROFILE --host TO --reason WHY`'s pass: deliver
/// the profile's programs to TO and stage its units, so TO becomes a place
/// it may move to. A pass that stops at a release rollout the host's agent
/// has yet to stage is refused with its detail, since nothing may move to TO
/// before its units are declared there.
pub(super) async fn standby(profile: &str, to: &str, reason: &str) -> Result<(), CmdError> {
    use crate::cli::placement::standby::{prepare, words};
    let report = prepare(profile, to, reason).await?;
    if report.outcome != words::DECLARED {
        return Err(CmdError::refused(format!(
            "placement standby of {profile} on {to} ended {}: {}; run the evacuation again once \
             it has",
            report.outcome, report.detail
        )));
    }
    Ok(())
}

/// Whether TO must be stood by for `profile` before it can move there: the
/// profile does not name TO, or names it while a unit file it declares there
/// is missing (a host written into the profile whose units were never
/// staged, or were removed since). Each unit file is asked of TO itself.
pub(super) async fn needs_standby(
    document: &Value,
    profile: &crate::placement::PlacementProfile,
    to: &str,
    runner: &Runner,
) -> Result<bool, CmdError> {
    let Some(host) = profile.hosts.get(to) else {
        return Ok(true);
    };
    let registry = crate::cli::placement::candidates::parse_registry(document)?;
    let target = crate::cli::placement::candidates::target(&registry, to)?;
    for unit in host.units.values().filter_map(|unit| unit.managed()) {
        let test = format!("-f {}", crate::deploy::shlex_quote(&unit.path));
        let present = crate::deploy::host_channel::remote_test(target, &test, runner)
            .await
            .map_err(CmdError::from)?;
        if !present {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether `profile` must be prepared in the run before it can move to TO:
/// it lacks a store the catalog declares for its products, or TO does not
/// fully stand by for it.
pub(super) async fn needs_preparing(
    document: &Value,
    profile: &crate::placement::PlacementProfile,
    to: &str,
    runner: &Runner,
) -> Result<bool, CmdError> {
    if !crate::cli::placement::profile::stores::lacking(profile)?.is_empty() {
        return Ok(true);
    }
    needs_standby(document, profile, to, runner).await
}

/// Profile `name` as the registry declares it now, with that registry.
async fn fresh(name: &str) -> Result<(Value, String, crate::placement::PlacementProfile), CmdError> {
    let (current, generation) = crate::cli::registry::fetch_versioned_document().await?;
    let profile = crate::placement::profiles(&current)
        .map_err(CmdError::declaration)?
        .into_iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| CmdError::missing(format!("the registry no longer declares profile {name}")))?;
    Ok((current, generation, profile))
}

/// What preparing one profile would do, for `--check`: the catalog stores
/// it lacks, and standby's own check when TO does not fully stand by for it
/// (`null` when it does, so a profile that only lacks stores is not shown a
/// standby it will not get); the refusal names which half refused.
pub(super) async fn check_preparing(
    document: &Value,
    profile: &crate::placement::PlacementProfile,
    to: &str,
    reason: &str,
    runner: &Runner,
) -> Result<Value, String> {
    use crate::cli::placement::profile::stores;
    let lacking = stores::lacking(profile).map_err(|error| format!("{}: reading its catalog stores: {error}", profile.name))?;
    let standing = needs_standby(document, profile, to, runner)
        .await
        .map_err(|error| format!("{}: whether {to} stands by: {error}", profile.name))?;
    let checked = if standing {
        crate::cli::placement::standby::check(&profile.name, to, reason)
            .await
            .map_err(|error| format!("{}: standing {to} by: {error}", profile.name))?
    } else {
        Value::Null
    };
    Ok(serde_json::json!({
        "profile": profile.name,
        "status": "needs_preparing",
        "to_host": to,
        "adds_stores": stores::listed(&lacking),
        "standby": checked,
    }))
}

/// Prepare one profile in the run: give it the stores the catalog declares
/// that it lacks (`profile sync`), stand TO by when TO does not fully
/// stand by for it (`placement standby`), then check its move from the
/// registry as it then stands.
pub(super) async fn prepare(name: &str, to: &str, reason: &str, runner: &Runner) -> Result<(), CmdError> {
    starting(format!("preparing {name} for {to}: the catalog stores it lacks"));
    crate::cli::placement::profile::stores::add_missing(name).await?;
    let (current, _, profile) = fresh(name).await?;
    if needs_standby(&current, &profile, to, runner).await? {
        starting(format!("standing {to} by for {name}: delivering its programs and staging its units"));
        standby(name, to, reason).await?;
    }
    let (current, generation, profile) = fresh(name).await?;
    super::super::check::preflight_move(current, generation, profile, to).await?;
    Ok(())
}

/// Move one profile from the registry as it stands now; its receipt, kept in
/// `placement history` with the refusal when it fails.
pub(super) async fn move_profile(name: &str, to: &str) -> Result<Value, CmdError> {
    use crate::cli::placement::history;
    let (current, generation, profile) = fresh(name).await?;
    starting(format!("moving {name} to {to}"));
    crate::cli::placement::candidates::ensure_profile_lifecycle_mutable(&profile)?;
    match super::super::moves::relocate(current, generation, profile, to).await {
        Ok(receipt) => {
            let report = receipt.report();
            history::record("move", Ok(&report));
            // The next step reaches the moved services through this host's resolver.
            crate::cli::resolver::follow_authority(false)
                .await
                .map_err(|error| error.also(format!("{name} itself moved to {to}; the run stopped before the next step")))?;
            Ok(report)
        }
        Err(error) => {
            history::record("move", Err(&error));
            Err(error)
        }
    }
}

/// `stado placement move --authority --to-host TO`, with `--check` unless
/// `apply`; this host must be TO, as the role is taken where it runs. When
/// the directory already names TO the authority (an evacuation run again
/// after a later step stopped it), there is nothing to take, and the step
/// says so instead of refusing the rerun.
pub(super) async fn authority(to: &str, apply: bool, runner: &Runner) -> Result<Value, CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    if super::scan::authority_holder(&document) == Some(to) {
        return Ok(serde_json::json!({ "status": "already_authority", "authority": to }));
    }
    let mut args = vec!["placement", "move", "--authority", "--to-host", to];
    if apply {
        starting(format!("taking the registry authority on {to}"));
    } else {
        args.push("--check");
    }
    let report = run_self(&args, runner).await?;
    // The steps after it read the registry this host now serves.
    if apply {
        crate::cli::resolver::follow_authority(false)
            .await
            .map_err(|error| error.also(format!("the registry authority itself was taken on {to}; the run stopped before the next step")))?;
    }
    Ok(report)
}

/// With `--authority` (run on TO): a registry authority that does not answer
/// commits no write, so it is taken on TO first and every other step goes
/// through TO; its receipt. A check cannot read through a silent authority,
/// so it refuses naming the silence; so does any run whose silent authority's
/// host also serves a role of the host Stado process, since the store a new
/// authority writes is that role's. `None` when the authority answers. Every
/// run starts here, so it also refuses an answering authority whose Stado
/// lacks `resolver follow`, which each moving step runs there.
pub(super) async fn authority_first(to: &str, asked: bool, check: bool) -> Result<Option<Value>, CmdError> {
    if !asked {
        crate::cli::resolver::ensure_upstream_follows().await?;
        return Ok(None);
    }
    crate::cli::placement::transfer::ensure_runs_on(to)?;
    let Err(silence) = crate::cli::resolver::authority_snapshot().await else {
        crate::cli::resolver::ensure_upstream_follows().await?;
        return Ok(None);
    };
    let copy = crate::cli::resolver::kept_last_good_document()
        .map_err(CmdError::declaration)?
        .ok_or_else(|| CmdError::refused("this host keeps no registry copy naming the silent authority"))?;
    // This host's registry store is the fleet object API when its storage is
    // the object API; a takeover writes it, so a silent host serving it stops it.
    let served_store = crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::StadoObject);
    if let Some(holder) = super::scan::authority_holder(&copy).filter(|_| served_store) {
        let roles = super::scan::fixed_routes_on(&copy, holder)?.roles;
        if !roles.is_empty() {
            return Err(CmdError::refused(format!(
                "the registry authority {holder} does not answer ({silence}) and also serves {}, the store a new \
                 authority would write, so neither can move without it; evacuate while {holder} answers",
                roles.join(", ")
            )));
        }
    }
    if check {
        return Err(CmdError::refused(format!(
            "the registry authority does not answer ({silence}), so no step can be checked through it; \
             run the evacuation without --check to take the authority on {to} first and check every other step through {to}"
        )));
    }
    authority(to, true, &crate::deploy::production_runner()).await.map(Some)
}

/// Stop releasing each of `products` to `from`, each one verified registry
/// write (`stado release policy target remove`); a product no longer
/// released there when the run reaches it (dropped meanwhile) is passed
/// over, and a refusal names the products dropped before it.
pub(super) async fn drop_release_targets(from: &str, products: &[String]) -> Result<Vec<Value>, CmdError> {
    let current = crate::cli::registry::fetch_document().await?;
    let still = super::scan::rolled_out_on(&current, from);
    let mut receipts = Vec::new();
    for product in products.iter().filter(|product| still.contains(product)) {
        starting(format!("stopping release control from releasing {product} to {from}"));
        match crate::cli::release_cmd::drop_policy_target(product, from).await {
            Ok(receipt) => receipts.push(receipt),
            Err(error) => {
                let dropped: Vec<String> = receipts.iter().filter_map(|kept| kept["product"].as_str()).map(str::to_string).collect();
                return Err(error.also(format!(
                    "release targets dropped from {from} before it: {}",
                    super::scan::listed(&dropped)
                )));
            }
        }
    }
    Ok(receipts)
}
