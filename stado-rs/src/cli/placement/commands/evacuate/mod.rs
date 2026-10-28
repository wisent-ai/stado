//! `stado placement evacuate --from-host A --to-host B --reason WHY
//! [--vault-owner-uid UID] [--authority] [--standby] [--profile-fixed-routes]
//! [--check] [--json]`: empty host A onto B as one operation that checks
//! every step before it takes any; with `--standby`, a profile that lacks a
//! catalog store or that B does not fully stand by for is prepared first,
//! and with `--profile-fixed-routes` each fixed route on A is given a
//! profile first, before anything moves. Once every step answered, the
//! registry is read again and must place nothing on A.
//!
//! Every step is checked first — the fleet vault (`credentials vault move`
//! without `--apply`), each profile's preflight, each role of the host Stado
//! process (`role move --check`), the registry authority (`placement move
//! --authority --check`, on B) and the install roots B would get — and one
//! refusal refuses the whole run before anything is claimed or fenced. Then
//! they run in order (roots under `{home}`, vault, profiles, the object API
//! carrying the authority), each from the registry the step before committed
//! and once this host's resolver follows it. A failed step stops the run
//! naming what moved and what still stands on A. A silent authority commits
//! nothing, so with `--authority` it is taken on B first.

mod apply;
mod fixed;
mod holds;
mod scan;
mod steps;
mod text;

pub(in crate::cli::placement) use holds::{holds, holds_everywhere};

use serde_json::{json, Value};

use self::scan::{fixed_routes_on, listed, placed_on};
use super::check::preflight_move;
use crate::cli::placement::transfer::prepare_role;
use crate::cli::{registry, CmdError};
use crate::deploy::production_runner;
use crate::placement::PlacementProfile;

/// What an evacuation is asked to do.
pub(in crate::cli::placement) struct Evacuation<'a> {
    pub from: &'a str,
    pub to: &'a str,
    pub reason: &'a str,
    /// The vault owner B's keyring holds, for the profile carrying the vault.
    pub vault_owner_uid: Option<&'a str>,
    /// Take the registry authority on B (with A's first role, or first when it is silent); run on B.
    pub authority: bool,
    pub check: bool,
    /// Stand B by for each profile that does not name it yet, in the run.
    pub standby: bool,
    /// Give each fixed route on A the profile `profile draft` prints first.
    pub profile_fixed: bool,
    /// Pin the schedules pinned to A to B once A is emptied.
    pub repin_schedules: bool,
    /// Stop releasing to A each release-control product that rolls out there.
    pub drop_release_targets: bool,
    /// Write install roots that would send B outside its home under `{home}` first.
    pub portable_roots: bool,
}

pub(in crate::cli::placement) async fn evacuate(
    asked: Evacuation<'_>,
    json_output: bool,
) -> Result<Value, CmdError> {
    let Evacuation {
        from,
        to,
        reason,
        vault_owner_uid,
        authority,
        check,
        standby,
        profile_fixed,
        repin_schedules,
        drop_release_targets,
        portable_roots,
    } = asked;
    if from == to {
        return Err(CmdError::click(format!(
            "--from-host and --to-host both name {from}"
        )));
    }
    let taken_first = steps::authority_first(to, authority, check).await?;
    // Fixed routes given a profile first are declared before what stands on
    // A is read, so this run carries them; a check only drafts them.
    let profiled = if profile_fixed {
        fixed::profile_fixed_routes(from, !check).await?
    } else {
        fixed::Profiled {
            drafted: Vec::new(),
            stays: Vec::new(),
        }
    };
    let (document, generation) = registry::fetch_versioned_document().await?;
    crate::targets::validate_registry(&document).map_err(CmdError::from)?;
    let placed = placed_on(&document, from)?;
    let fixed = fixed_routes_on(&document, from)?;
    // A's roles store the registry when A is the authority, so none takes a
    // commit once fenced: the first carries the authority (role move --with-authority).
    let carried = authority
        && taken_first.is_none()
        && !fixed.roles.is_empty()
        && scan::authority_holder(&document) == Some(from);
    if placed.profiles.is_empty()
        && fixed.roles.is_empty()
        && !authority
        && profiled.drafted.is_empty()
        && !repin_schedules
        && !drop_release_targets
    {
        return Err(CmdError::missing(format!(
            "no placement profile and no role of the host Stado process is placed on {from}; \
             fixed routes active there: {}; profiles whose placement cannot be told: {}",
            listed(&fixed.routes),
            listed(&placed.untold)
        )));
    }
    // The vault's ciphertext moves with its profile, its owner's secret key
    // does not: `credentials vault move` rotates the owner, then moves it.
    let vault = crate::cli::secrets::VAULT_SERVICE;
    let (vaulted, profiles): (Vec<PlacementProfile>, Vec<PlacementProfile>) = placed
        .profiles
        .into_iter()
        .partition(|profile| profile.services.iter().any(|service| service == vault));
    let runner = production_runner();
    let mut refusals = Vec::new();
    let mut vault_report = Value::Null;
    // With --standby, a profile that lacks a catalog store or that B does not
    // fully stand by for is prepared in the run and checked there; here
    // standby's own check runs instead, and the vault move's waits for the run.
    let mut unstaged: Vec<&PlacementProfile> = Vec::new();
    let mut ready: Vec<&PlacementProfile> = Vec::new();
    for profile in vaulted.iter().chain(profiles.iter()) {
        if standby && steps::needs_preparing(&document, profile, to, &runner).await? {
            unstaged.push(profile);
        } else if !vaulted.iter().any(|kept| kept.name == profile.name) {
            ready.push(profile);
        }
    }
    let vault_unstaged = vaulted
        .first()
        .is_some_and(|profile| unstaged.iter().any(|kept| kept.name == profile.name));
    match (vaulted.first(), vault_owner_uid) {
        (None, _) => {}
        (Some(profile), None) => refusals.push(format!(
            "{}: it carries {vault}, the fleet vault, which moves with its owner rotated to a key \
             {to} holds; name that owner with --vault-owner-uid UID",
            profile.name
        )),
        (Some(_), Some(_)) if vault_unstaged => {}
        (Some(_), Some(uid)) => match steps::vault(to, uid, false, &runner).await {
            Ok(report) => vault_report = report,
            Err(error) => refusals.push(format!("{vault}: {error}")),
        },
    }
    let checks = ready.iter().map(|profile| {
        preflight_move(document.clone(), generation.clone(), (*profile).clone(), to)
    });
    let role_checks = fixed
        .roles
        .iter()
        .map(|role| prepare_role(role, to, carried, &runner));
    let (checks, role_checks) = futures::join!(
        futures::future::join_all(checks),
        futures::future::join_all(role_checks)
    );
    let mut reports = Vec::new();
    for (profile, answer) in ready.iter().zip(checks) {
        match answer {
            Ok(answer) => reports.push(answer.report()),
            Err(error) => refusals.push(format!("{}: {error}", profile.name)),
        }
    }
    for profile in &unstaged {
        match steps::check_preparing(&document, profile, to, reason, &runner).await {
            Ok(report) => reports.push(report),
            Err(refusal) => refusals.push(refusal),
        }
    }
    let mut role_reports = Vec::new();
    for (role, answer) in fixed.roles.iter().zip(role_checks) {
        match answer {
            Ok(plan) => role_reports.push(plan.report()),
            Err(error) => refusals.push(format!("{role}: {error}")),
        }
    }
    let mut authority_report = json!(taken_first);
    if authority && taken_first.is_none() && !carried {
        match steps::authority(to, false, &runner).await {
            Ok(report) => authority_report = report,
            Err(error) => refusals.push(format!("the registry authority: {error}")),
        }
    }
    let schedules =
        holds::schedules_to_repin(&document, from, repin_schedules, &mut refusals).await;
    // B must install every release it is a target of under its own home.
    let portable = scan::foreign_roots_for(&document, to, &unstaged, portable_roots, &mut refusals);
    let release_drops = scan::release_drops(
        &document,
        from,
        drop_release_targets,
        &unstaged,
        standby.then_some(&profiled),
        &mut refusals,
    );
    if !refusals.is_empty() {
        let moved = if taken_first.is_some() {
            format!("only the registry authority, which did not answer, was taken on {to}")
        } else {
            "nothing was moved".to_string()
        };
        return Err(CmdError::refused(format!(
            "evacuating {from} to {to} would be refused, so {moved}: {}",
            refusals.join("; ")
        )));
    }
    let mut answered = apply::Reports {
        portable: json!(portable),
        vault: vault_report,
        profiles: reports,
        roles: role_reports,
        authority: authority_report,
        schedules,
        release: json!(release_drops),
    };
    if !check {
        let plan = apply::Plan {
            from,
            to,
            reason,
            vault_owner_uid,
            authority,
            carried,
            taken_first: taken_first.is_some(),
            repin_schedules,
            document: &document,
            portable: &portable,
            unstaged: &unstaged,
            vaulted: &vaulted,
            profiles: &profiles,
            roles: &fixed.roles,
            release_drops: &release_drops,
            runner: &runner,
        };
        apply::run(&plan, &mut answered).await?;
    }
    let status = if check { "would_evacuate" } else { "evacuated" };
    let report = json!({
        "status": status,
        "from_host": from,
        "to_host": to,
        "vault": answered.vault,
        "profiles": answered.profiles,
        "roles": answered.roles,
        "verified_emptied": !check,
        "repinned_schedules": answered.schedules,
        "release_targets_dropped": answered.release,
        "portable_roots": answered.portable,
        "authority": answered.authority,
        "fixed_routes": fixed.routes,
        "drafted_profiles": profiled.drafted,
        "undrafted_routes": profiled.stays,
        "untold_profiles": placed.untold,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(report);
    }
    text::print(&text::Said {
        from,
        to,
        check,
        portable: &portable,
        unstaged: &unstaged,
        vaulted: &vaulted,
        profiles: &profiles,
        roles: &fixed.roles,
        authority,
        authority_report: &answered.authority,
        repin_schedules,
        schedules: &answered.schedules,
        drop_release_targets,
        release_drops: &release_drops,
        profiled: &profiled,
        routes: &fixed.routes,
        untold: &placed.untold,
    });
    Ok(report)
}
