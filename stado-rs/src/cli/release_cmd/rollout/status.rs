//! `stado release status` — desired rollout state against what each host
//! observes and against what it actually runs.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::release_control::{self, ProductReleasePolicy};

use super::ReleaseStatusArgs;

/// The binary one release policy installs on a host, as the concrete file the
/// host's software report names.
///
/// The rollout's own artefact lives under the product install root, so it is in
/// no `managed_versions` entry and in no `$HOME/.stado/bin` listing. A status
/// command that did not resolve this path could compare the desired version
/// against nothing at all — which is what `observed=unreported` was.
fn product_binary(policy: &ProductReleasePolicy) -> crate::host_software::ProductBinary {
    let path = format!(
        "{}/{}",
        policy.install_root.trim_end_matches('/'),
        policy.binary.trim_start_matches('/')
    );
    crate::host_software::ProductBinary {
        name: path
            .rsplit('/')
            .next()
            .unwrap_or(policy.binary.as_str())
            .to_string(),
        path,
        desired: policy
            .desired
            .as_ref()
            .map(|desired| desired.version.clone()),
    }
}

/// `stado release status [--product NAME] [--json]` — desired against observed,
/// and now against what the host actually runs.
///
/// Two things were true of this command until 2026-08-18 and both were wrong.
/// It printed `brama target=control-host desired=0.2.27 observed=unreported`
/// and exited **zero**, so a host that had never once said what it runs read as
/// a host with nothing to answer for. And `observed` came only from the release
/// agent's own state file, so software installed outside the release channel —
/// skarbiec 0.2.1 on one machine and 0.2.3 on another, neither in any published
/// release — was invisible here even while it was the thing breaking the fleet.
///
/// The third column closes both. It is the host's own software report
/// ([`crate::host_software`]), read out of the observation store rather than
/// gathered here: a status read must not cost one ssh connection per target, and
/// the age of the report is itself the finding when nobody has refreshed it.
/// Missing, stale, `unmanaged` and disagreeing are all failures, and the command
/// exits non-zero on any of them with one sentence per row naming the host and
/// the exact disagreement.
pub(in crate::cli::release_cmd) async fn status(args: &ReleaseStatusArgs) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    // One read of the observation file for the whole rendering, for the reason
    // `observations::describe_in` exists: a column whose cost scales with the
    // size of the fleet is a column somebody eventually deletes.
    let records = crate::observations::load();
    let mut reports = Vec::new();
    let mut failures = usize::default();
    for (product, policy) in &control.products {
        if args
            .product
            .as_deref()
            .is_some_and(|selected| selected != product)
        {
            continue;
        }
        for target in policy.targets.keys() {
            let uri = crate::release_agent::release_status_uri(product, target);
            let observed: Value = match crate::cli::storage::fetch_object(&uri).await {
                Ok(bytes) => serde_json::from_slice(&bytes)?,
                Err(_) => Value::Null,
            };
            let software = crate::host_software::load_in(&records, target);
            let declared = registry
                .targets
                .iter()
                .find(|entry| &entry.name == target)
                .map(|entry| entry.managed_versions.clone())
                .unwrap_or_default();
            let finding =
                crate::host_software::judge(&software, &declared, Some(&product_binary(policy)));
            if finding.failed {
                failures = failures.saturating_add(1);
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
    let runs = crate::cli::release_submit::recent_runs(args.product.as_deref(), 10).await?;
    if reports.is_empty() && runs.is_empty() {
        return Err(CmdError::click("no matching release product"));
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"targets": reports, "runs": runs}))?
        );
    } else {
        for report in &reports {
            let software = &report["software"];
            println!(
                "{} target={} desired={} observed={} software={} ({} of {} unmanaged, reported {})",
                report["product"].as_str().unwrap_or_default(),
                report["target"].as_str().unwrap_or_default(),
                report["desired"]["version"].as_str().unwrap_or("-"),
                report["observed"]["phase"].as_str().unwrap_or("unreported"),
                software["verdict"].as_str().unwrap_or("failed"),
                software["unmanaged"].as_u64().unwrap_or_default(),
                software["reported"].as_u64().unwrap_or_default(),
                software["observed"].as_str().unwrap_or("never"),
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
        if !runs.is_empty() {
            println!("--- pipeline runs (newest first):");
        }
        for run in runs {
            println!(
                "run {} {} {} {} {} {}",
                &run["run_id"].as_str().unwrap_or("-")
                    [..8.min(run["run_id"].as_str().unwrap_or("-").len())],
                run["product"].as_str().unwrap_or("-"),
                run["version"].as_str().unwrap_or("-"),
                run["channel"].as_str().unwrap_or("-"),
                run["state"].as_str().unwrap_or("-"),
                run["updated_at"].as_str().unwrap_or("-"),
            );
            // Each platform on its own line: the run-level state alone reads
            // as a promise, while "linux-amd64 submitted job=4ffae52f
            // [running]" is a fact an operator can go and watch.
            for (platform, record) in run["platforms"].as_object().into_iter().flatten() {
                let mut line = format!(
                    "  {platform} {} job={}",
                    record["state"].as_str().unwrap_or("-"),
                    &record["job_id"].as_str().unwrap_or("-")
                        [..8.min(record["job_id"].as_str().unwrap_or("-").len())],
                );
                if let Some(job_state) = record["job_state"].as_str() {
                    line.push_str(&format!(" [{job_state}]"));
                }
                // An estimate and labelled as one: crates compiled so far
                // against this platform's previous run, because cargo
                // publishes no total of its own.
                if let Some(compiled) = record["compile_progress"]["compiled"].as_u64() {
                    match record["compile_progress"]["percent"].as_u64() {
                        Some(percent) => line.push_str(&format!(
                            " compiled {compiled} crates (~{percent}% of the previous run)"
                        )),
                        None => line.push_str(&format!(" compiled {compiled} crates")),
                    }
                }
                println!("{line}");
                if let Some(failure) = record["failure"].as_str() {
                    println!("    failure: {}", failure.lines().next().unwrap_or(failure));
                }
            }
            if let Some(failure) = run["failure"].as_str() {
                // One line of evidence, not the whole log: the first line
                // names the failing step and host; `submit --json` carries
                // the rest.
                println!("  failure: {}", failure.lines().next().unwrap_or(failure));
            }
        }
    }
    if failures == usize::default() {
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
