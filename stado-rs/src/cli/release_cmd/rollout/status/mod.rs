//! `stado release status` — desired rollout state against what each host
//! observes and against what it actually runs.

mod reads;
mod render;

use serde_json::{json, Value};

use crate::cli::release_submit::RunFilter;
use crate::cli::CmdError;
use crate::host_software::ProductBinary;
use crate::release_control;

use super::ReleaseStatusArgs;
use reads::{matching_runs, recent_runs};
use render::print_runs;

/// `stado release status [--product NAME] [--json]` — desired against observed,
/// and now against what the host actually runs.
///
/// Two things used to be true of this command and both were wrong.
/// It printed `desired=<version> observed=unreported`
/// and exited **zero**, so a host that had never once said what it runs read as
/// a host with nothing to answer for. And `observed` came only from the release
/// agent's own state file, so software installed outside the release channel —
/// a vault binary at a version no published release carried — was invisible
/// here even while it was the thing breaking the fleet.
///
/// The third column closes both. It is the host's own software report
/// ([`crate::host_software`]), read out of the observation store rather than
/// gathered here: a status read must not cost one ssh connection per target, and
/// the age of the report is itself the finding when nobody has refreshed it.
/// Missing, stale, `unmanaged` and disagreeing are all failures, and the command
/// exits non-zero on any of them with one sentence per row naming the host and
/// the exact disagreement.
pub(in crate::cli::release_cmd) async fn status(args: &ReleaseStatusArgs) -> Result<(), CmdError> {
    // A question about one run reads that run, instead of the whole status
    // being written to a file and one version's block grepped out of it.
    if args.run.is_some() || args.version.is_some() {
        return runs_only(args).await;
    }
    let document = reads::registry_document().await?;
    let control = release_control::control(&document)?.ok_or_else(|| {
        CmdError::click("registry.release_control is not configured")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let registry = reads::registry_targets().await?;
    // One read of the observation file for the whole rendering, for the reason
    // `observations::describe_in` exists: a column whose cost scales with the
    // size of the fleet is a column somebody eventually deletes.
    let records = crate::observations::load();
    let mut reports = Vec::new();
    // Rollout targets whose software is judged failed; each is a failure.
    let mut failed: Vec<&String> = Vec::new();
    // Rollout targets the fleet registry does not declare; each is a failure.
    let mut undeclared: Vec<&String> = Vec::new();
    let selected = |product: &str| {
        args.product
            .as_deref()
            .is_none_or(|chosen| chosen == product)
    };
    let pairs: Vec<(String, String)> = control
        .products
        .iter()
        .filter(|(product, _)| selected(product))
        .flat_map(|(product, policy)| {
            policy
                .targets
                .keys()
                .map(|target| (product.clone(), target.clone()))
        })
        .collect();
    let mut states = reads::release_states(&pairs).await?;
    for (product, policy) in control
        .products
        .iter()
        .filter(|(product, _)| selected(product))
    {
        for target in policy.targets.keys() {
            let state = states
                .remove(&(product.clone(), target.clone()))
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "the release state of {product} on {target} was not read"
                    ))
                })??;
            let observed: Value = match state {
                Some(bytes) => serde_json::from_slice(&bytes)?,
                None => Value::Null,
            };
            let software = crate::host_software::load_in(&records, target);
            // A rollout target the registry does not declare has no managed
            // versions to judge against. Its software block is the host's own
            // report, as every target's is, with the one finding that nothing
            // could be judged folded in: one block shape for every reader.
            let Some(entry) = registry.targets.iter().find(|entry| &entry.name == target) else {
                undeclared.push(target);
                let mut block = software.json();
                crate::host_software::Finding::undeclared(product, target).merge_into(&mut block);
                reports.push(json!({
                    "product": product,
                    "target": target,
                    "desired": policy.desired,
                    "previous": policy.previous,
                    "observed": observed,
                    "software": block,
                }));
                continue;
            };
            let finding = crate::host_software::judge(
                &software,
                &entry.managed_versions,
                Some(&ProductBinary::of(policy)),
            );
            if finding.failed {
                failed.push(target);
            }
            let mut block = software.json();
            finding.merge_into(&mut block);
            reports.push(json!({
                "product": product,
                "target": target,
                "desired": policy.desired,
                "previous": policy.previous,
                "observed": observed,
                "software": block,
            }));
        }
    }
    let failures = failed.len().saturating_add(undeclared.len());
    let runs = recent_runs(args.product.as_deref(), run_window(args)?).await?;
    if reports.is_empty() && runs.is_empty() {
        // The request names nothing Stado holds: a refusal of the request,
        // with what is configured, never an unattributed failure.
        let refused = crate::primitives::failure::FailureCode::Refused;
        return Err(match args.product.as_deref() {
            Some(product) if !control.products.contains_key(product) => {
                crate::cli::release_cmd::unknown_release_product(&control, product)
            }
            Some(product) => CmdError::click(format!(
                "release control holds {product:?} with no rollout target, and none of the {} \
                 release runs read is its",
                window_words(args)
            ))
            .stating(refused),
            None => CmdError::click(format!(
                "release control has no rollout target and there is no release run among the {}",
                window_words(args)
            ))
            .stating(refused),
        });
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"targets": reports, "runs": runs}))?
        );
    } else {
        for report in &reports {
            let software = &report["software"];
            // A target whose report carries no software verdict or counts is
            // said to report none, never as a failed verdict over counts of
            // zero.
            let software_line = match (
                software["verdict"].as_str(),
                software["unmanaged"].as_u64(),
                software["reported"].as_u64(),
            ) {
                (Some(verdict), Some(unmanaged), Some(reported)) => format!(
                    "{verdict} ({unmanaged} of {reported} unmanaged, reported {})",
                    match software["observed"].as_str() {
                        Some(observed) => observed,
                        None => "never",
                    }
                ),
                _ => crate::cli::reporting::NOT_REPORTED.to_string(),
            };
            let desired = match report["desired"]["version"].as_str() {
                Some(version) => version,
                None => "(none declared)",
            };
            // A row without its product or target says so: an empty word
            // stood where the row's subject was.
            let named = |field: &str| match report[field].as_str() {
                Some(name) => name.to_string(),
                None => format!("(no {field} reported)"),
            };
            let observed = match report["observed"]["phase"].as_str() {
                Some(phase) => phase,
                None => "unreported",
            };
            println!(
                "{} target={} desired={} observed={} software={}",
                named("product"),
                named("target"),
                desired,
                observed,
                software_line,
            );
            // One sentence per row, on stdout beside the row it is about: an
            // operator reading a gate should not have to interleave two streams
            // to learn which target the accusation belongs to.
            for sentence in software["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                println!("  ! {sentence}");
            }
        }
        print_runs(&runs);
    }
    if failed.is_empty() && undeclared.is_empty() {
        return Ok(());
    }
    // Silence is the failure. Every sentence is already printed beside its row,
    // so this exits without a second message: what a gate owes an operator is a
    // non-zero code and the reason, not the reason twice.
    eprintln!(
        "{failures} of {} rollout target(s) cannot be shown to be running what the fleet declares \
         for them; each is named above",
        reports.len()
    );
    Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE))
}

/// The runs one listing reads: the newest `--limit`, or every recorded run.
/// A limit this platform cannot count to is refused rather than cut down.
fn run_window(args: &ReleaseStatusArgs) -> Result<usize, CmdError> {
    match args.limit {
        Some(limit) => usize::try_from(limit.get()).map_err(|error| {
            CmdError::usage(format!("--limit {limit} cannot be counted here: {error}"))
        }),
        None => Ok(usize::MAX),
    }
}

/// How a refusal names the runs it read.
fn window_words(args: &ReleaseStatusArgs) -> String {
    match args.limit {
        Some(limit) => format!("newest {limit}"),
        None => "recorded".to_string(),
    }
}

/// How a refusal names one filter the request did not set: any value.
fn filter_word(value: Option<&str>) -> &str {
    match value {
        Some(value) => value,
        None => "*",
    }
}

/// `release status --run ID | --version V`: the runs the filter admits, and
/// a refusal naming the runs read when none does. With `--wait` the read
/// holds, on the store's change watch over the named run's own record, until
/// the one run `--run` names has stopped moving, and a run that finished
/// without publishing ends the command as failed, so `--wait && install`
/// installs only a published release.
async fn runs_only(args: &ReleaseStatusArgs) -> Result<(), CmdError> {
    let filter = RunFilter {
        product: args.product.as_deref(),
        run: args.run.as_deref(),
        version: args.version.as_deref(),
    };
    let mut runs = matching_runs(filter, run_window(args)?).await?;
    if args.wait {
        let store = crate::queue::JobStorage::new()
            .await
            .map_err(CmdError::from)?;
        // A run's record is `runs/release-pipeline/<id>/run.json`, rewritten
        // in place: a watch on the folder of every run sees a run created and
        // never a run's record rewritten, so the wait never woke. The watch is
        // on the named run's own folder, armed once the read has named the
        // run and followed by a second read, so a write between the two is
        // read rather than waited for.
        while let Some(run_id) = unfinished(&runs)? {
            let folder = format!("{RUN_RECORDS}{run_id}");
            let armed = store.watch_prefixes(&[&folder]).map_err(|error| {
                CmdError::click(format!("release status --wait cannot hold: {error}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
            runs = matching_runs(filter, run_window(args)?).await?;
            let Some(state) = still_moving(&runs, &run_id)? else {
                break;
            };
            eprintln!("stado release status: run {run_id} is {state}; waiting for it to finish");
            run_changed(armed).await?;
            runs = matching_runs(filter, run_window(args)?).await?;
        }
    }
    if runs.is_empty() {
        let recent = recent_runs(args.product.as_deref(), run_window(args)?).await?;
        let known: Vec<String> = recent
            .iter()
            .map(|run| {
                let field = |name: &str| match run[name].as_str() {
                    Some(value) => value.to_string(),
                    None => format!("(no {name})"),
                };
                format!(
                    "{} {} {}",
                    field("run_id"),
                    field("product"),
                    field("version")
                )
            })
            .collect();
        return Err(CmdError::click(format!(
            "no release run matches run={} version={} product={}; the {} are:\n  {}",
            filter_word(args.run.as_deref()),
            filter_word(args.version.as_deref()),
            filter_word(args.product.as_deref()),
            window_words(args),
            known.join("\n  ")
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "runs": runs }))?
        );
    } else {
        print_runs(&runs);
    }
    if args.wait {
        unpublished(&runs)?;
    }
    Ok(())
}

/// Where every release run record is kept; `--wait` watches one run's folder
/// under it.
const RUN_RECORDS: &str = "runs/release-pipeline/";

/// The id of the run `--run` named while it has not finished, or `None` once
/// it has (or when no run matched, which the caller refuses). More than one
/// run matching a `--run` prefix is refused: the wait would not know which to
/// follow.
fn unfinished(runs: &[Value]) -> Result<Option<String>, CmdError> {
    let run = match runs {
        [] => return Ok(None),
        [run] => run,
        several => {
            return Err(CmdError::refused(format!(
                "--run names {} runs ({}); --wait follows one, so name it by more of its id",
                several.len(),
                several
                    .iter()
                    .filter_map(|run| run["run_id"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
    };
    let id = run["run_id"].as_str().unwrap_or_default();
    Ok(still_moving(runs, id)?.map(|_| id.to_string()))
}

/// The state word of run `id` among `runs` while it has not finished, or
/// `None` once it has or is no longer among them.
fn still_moving(runs: &[Value], id: &str) -> Result<Option<String>, CmdError> {
    let Some(run) = runs.iter().find(|run| run["run_id"].as_str() == Some(id)) else {
        return Ok(None);
    };
    let word = run["state"].as_str().unwrap_or_default();
    let state = crate::release_pipeline::ReleaseRunState::named(word).ok_or_else(|| {
        CmdError::click(format!(
            "run {id} records state {word:?}, which this Stado does not know, so whether it \
             finished cannot be read"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    Ok((!state.finished()).then(|| word.to_string()))
}

/// Refuse a finished run that published nothing (failed, or superseded
/// before it published), so a caller chaining an install after `--wait`
/// does not install over it.
fn unpublished(runs: &[Value]) -> Result<(), CmdError> {
    let Some(run) = runs.first() else {
        return Ok(());
    };
    let word = run["state"].as_str().unwrap_or_default();
    let published = crate::release_pipeline::ReleaseRunState::named(word)
        .is_some_and(|state| state.published());
    if published {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "run {} finished {word} without publishing: {}",
        run["run_id"].as_str().unwrap_or_default(),
        run["failure"].as_str().unwrap_or("it recorded no failure")
    )))
}

/// Wait on `armed` until a run record changes.
async fn run_changed(
    armed: Box<dyn crate::queue::ChangeWatch>,
) -> Result<Box<dyn crate::queue::ChangeWatch>, CmdError> {
    tokio::task::spawn_blocking(move || {
        let mut armed = armed;
        armed.next().map(|()| armed)
    })
    .await
    .map_err(|error| {
        CmdError::click(format!("the change watch on the release runs stopped: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?
    .map_err(|error| {
        CmdError::click(format!("the change watch on the release runs failed: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}
