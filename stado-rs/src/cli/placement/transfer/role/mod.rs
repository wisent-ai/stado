//! `stado placement role move SERVICE --to-host HOST --reason WHY`: move a
//! directory service the host Stado process serves (the fleet object API)
//! to another host, while the source's Stado keeps serving its other roles.
//!
//! A placement move stops and starts units, and the host Stado process runs
//! on every host, so it cannot move one of that process's roles. What such a
//! role carries is its store: `~/.stado/local-storage`, which every host's
//! Stado serves its object API from. The role moves as its store and its
//! route:
//!
//! 1. the source's store is fenced for the transaction (`stado storage fence
//!    arm` on the source), which returns once every write in flight finished;
//! 2. the store is copied whole to this machine and put in place on the
//!    destination, its previous tree kept as the move's backup;
//! 3. one registry commit names the destination as the route's active host
//!    ([`route::switched`]).
//!
//! A step that fails undoes what came before it: the destination's tree is
//! put back and the source's fence lifted, so the source serves as before.
//! After the commit the source's copy stays fenced: a write there would be
//! lost to the store that now serves, and the fence names the transaction
//! that lifts it.

mod carry;
mod fence;
mod route;

pub(in crate::cli::placement) use carry::ensure_runs_on;
pub(in crate::cli::placement) use fence::store_fence;

use serde_json::{json, Value};

use self::fence::{fence, left_fence};
use self::route::{checked_route, served_address, switched};
use super::settle_state_backup;
use super::state::{ensure_room_for, read_state, restore_state, write_state, Ends};
use crate::cli::placement::candidates::{parse_registry, target};
use crate::cli::{registry, CmdError};
use crate::deploy::{production_runner, Runner};
use crate::placement::{PlacementState, StateRoot};
use crate::targets::ComputeTarget;

/// The state `stado storage fence arm --json` answers once the fence holds.
const ARMED: &str = "armed";

/// The store a host-process role carries, relative to the account's home.
fn store() -> PlacementState {
    PlacementState {
        path: crate::config_file::LOCAL_STORAGE_ROOT.to_string(),
        required: true,
        root: StateRoot::Home,
        tree: true,
    }
}

/// Undo the copy on `destination` and lift the source's fence; what failed
/// of either is added to `primary`.
async fn undo(
    primary: CmdError,
    source: &ComputeTarget,
    destination: Option<&ComputeTarget>,
    transaction: &str,
    runner: &Runner,
) -> CmdError {
    let mut failure = primary;
    if let Some(destination) = destination {
        if let Err(error) = restore_state(destination, &store(), transaction, runner).await {
            failure = failure.also(format!("putting {}'s store back failed: {error}", destination.name));
        }
    }
    // `disarmed`, or `not-fenced` when the arm never landed: either way the
    // source takes writes again.
    let lifted = fence(source, "disarm", transaction, "role move undone", runner).await;
    if let Err(error) = lifted {
        failure = failure.also(format!(
            "lifting {}'s write fence failed, so it refuses writes until stado storage fence \
             disarm --transaction {transaction} runs there: {error}",
            source.name
        ));
    }
    failure
}

/// One role move checked against the registry and the hosts' room, ready
/// to run.
pub(in crate::cli::placement) struct RoleMove {
    service: String,
    to_host: String,
    source: ComputeTarget,
    destination: ComputeTarget,
    served: String,
    /// The fence an earlier role move left on the destination's store, which
    /// this move lifts once it commits.
    left: Option<String>,
    /// The registry authority moves in the same registry as the route,
    /// because the source holds it and its store is the one being fenced.
    with_authority: bool,
}

impl RoleMove {
    /// What the check found, as `--check --json` prints it.
    pub(in crate::cli::placement) fn report(&self) -> Value {
        json!({
            "state": "would_move",
            "service": self.service,
            "from_host": self.source.name,
            "to_host": self.to_host,
            "endpoint": self.served,
            "with_authority": self.with_authority,
        })
    }
}

/// Check moving `service` to `to_host` as the registry stands now: the
/// route, and room for the store on the destination and on this machine,
/// which the copy passes through. Nothing is fenced or changed.
pub(in crate::cli::placement) async fn prepare_role(
    service: &str,
    to_host: &str,
    with_authority: bool,
    runner: &Runner,
) -> Result<RoleMove, CmdError> {
    let document = registry::fetch_document().await?;
    let source_name = checked_route(&document, service, to_host)?;
    carry::ensure_carriable(&document, service, &source_name, to_host, with_authority)?;
    let parsed = parse_registry(&document)?;
    let source = target(&parsed, &source_name)?.clone();
    if with_authority {
        carry::ensure_source_takes_fenced_commit(&document, &source, runner).await?;
    }
    let destination = target(&parsed, to_host)?.clone();
    let served = served_address()?;
    if with_authority {
        carry::ensure_destination_store_answers(&served, to_host).await?;
    }
    let ends = Ends {
        source: &source,
        destination: &destination,
        registry: &parsed,
    };
    ensure_room_for(&ends, &[store()], runner).await?;
    let left = left_fence(&destination, runner).await?;
    Ok(RoleMove {
        service: service.to_string(),
        to_host: to_host.to_string(),
        source,
        destination,
        served,
        left,
        with_authority,
    })
}

/// Run a checked role move: fence, copy, switch the route, settle the
/// destination's backup; a failed step puts both hosts back. The receipt,
/// kept in the placement history with a refusal as with a success.
pub(in crate::cli::placement) async fn apply_role(
    plan: &RoleMove,
    reason: &str,
    runner: &Runner,
) -> Result<Value, CmdError> {
    let outcome = role_steps(plan, reason, runner).await;
    crate::cli::placement::history::record("move-role", outcome.as_ref());
    outcome
}

async fn role_steps(plan: &RoleMove, reason: &str, runner: &Runner) -> Result<Value, CmdError> {
    let RoleMove { service, to_host, source, destination, served, left, with_authority } = plan;
    let source_name = &source.name;
    let transaction = uuid::Uuid::new_v4().to_string();
    let why = format!("placement role move {service} to {to_host}: {reason}");
    let armed = fence(source, "arm", &transaction, &why, runner).await?;
    if armed != ARMED {
        let error = CmdError::click(format!(
            "{source_name}: stado storage fence arm answered {armed}, not {ARMED}"
        ));
        return Err(undo(error, source, None, &transaction, runner).await);
    }
    let snapshot = match read_state(source, &store(), &transaction, runner).await {
        Ok(snapshot) => snapshot,
        Err(error) => return Err(undo(error, source, None, &transaction, runner).await),
    };
    if let Err(error) = write_state(destination, &snapshot, &transaction, runner).await {
        return Err(undo(error, source, Some(destination), &transaction, runner).await);
    }
    // A source that holds the authority cannot take a commit once fenced:
    // the route and the authority are installed together on the destination,
    // into the store just copied there, so a fence an earlier move left on it
    // is lifted first and put back when nothing was installed.
    let mut lifted_first = None;
    let (generation, authority) = if *with_authority {
        if let Some(earlier) = left {
            match fence(destination, "disarm", earlier, &why, runner).await {
                Ok(state) => lifted_first = Some(json!({"transaction": earlier, "state": state})),
                Err(error) => return Err(undo(error, source, Some(destination), &transaction, runner).await),
            }
        }
        match carry::carried(service, source_name, to_host, served, &transaction).await {
            Ok(installed) => (installed.receipt["directory_generation"].clone(), Some(installed)),
            Err(error) => {
                let mut error = undo(error, source, Some(destination), &transaction, runner).await;
                if let Some(earlier) = left {
                    if let Err(rearm) = fence(destination, "arm", earlier, &why, runner).await {
                        error = error.also(format!("{to_host}'s earlier fence {earlier} was not put back: {rearm}"));
                    }
                }
                return Err(error);
            }
        }
    } else {
        let committed = registry::commit_document(|current| switched(current, service, source_name, to_host, served)).await;
        match committed {
            Ok(generation) => (json!(generation), None),
            Err(error) => return Err(undo(error, source, Some(destination), &transaction, runner).await),
        }
    };
    let (kept_store, unsettled_store) =
        match settle_state_backup(destination, &store(), &transaction, runner).await {
            Ok(kept) => (json!(kept), Value::Null),
            Err(error) => (Value::Null, json!(error.to_string())),
        };
    // A store an earlier role move fenced when it left this host serves
    // again now: its fence is lifted, or it would refuse every write. The
    // carried path lifted it before writing the registry there.
    let lifted = match (left, lifted_first) {
        (_, Some(answered)) => answered,
        (None, None) => Value::Null,
        (Some(earlier), None) => match fence(destination, "disarm", earlier, &why, runner).await {
            Ok(state) => json!({"transaction": earlier, "state": state}),
            Err(error) => json!({"transaction": earlier, "error": error.to_string()}),
        },
    };
    let receipt = json!({
        "state": "moved",
        "service": service,
        "from_host": source_name,
        "to_host": to_host,
        "transaction": transaction,
        "registry_generation": generation,
        "kept_replaced_store": kept_store,
        "unsettled_store": unsettled_store,
        "source_fenced": true,
        "destination_fence_lifted": lifted,
        "authority": authority.as_ref().map(|installed| installed.receipt.clone()),
    });
    // The route and the authority stand on the destination now; a host or
    // store the install did not reach is named, not undone.
    match authority.and_then(|installed| installed.failure) {
        Some(failure) => Err(failure.also(format!(
            "{service} moved to {to_host} with the registry authority, and {to_host}'s copy holds the registry; write it into the \
             store with stado registry pull --last-known-good > registry.json and stado registry push registry.json; receipt: {receipt}"
        ))),
        None => Ok(receipt),
    }
}

pub(in crate::cli::placement) async fn move_role(
    service: &str,
    to_host: &str,
    reason: &str,
    with_authority: bool,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let runner = production_runner();
    let plan = prepare_role(service, to_host, with_authority, &runner).await?;
    let receipt = if check { plan.report() } else { apply_role(&plan, reason, &runner).await? };
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
        return Ok(());
    }
    let from = &plan.source.name;
    if check {
        let carried = if with_authority { ", carrying the registry authority" } else { "" };
        println!(
            "{service} can move from {from} to {to_host}{carried}: the route is a role of the host Stado \
             process and the store fits on both {to_host} and this machine; nothing was fenced or \
             changed"
        );
        return Ok(());
    }
    use crate::cli::reporting::field;
    println!(
        "moved {service} from {from} to {to_host} (registry generation {}); {from}'s copy stays \
         fenced by transaction {}",
        field(&receipt["registry_generation"]),
        field(&receipt["transaction"])
    );
    if with_authority {
        println!("  {to_host} holds the registry authority from the same registry");
    }
    if let Some(path) = receipt["kept_replaced_store"].as_str() {
        println!("  kept what {to_host} held there before the move: {path}");
    }
    if let Some(error) = receipt["unsettled_store"].as_str() {
        eprintln!("  {to_host}: the store's backup stays unsettled: {error}");
    }
    Ok(())
}
