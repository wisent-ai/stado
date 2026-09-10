//! `stado release doctor` — one verdict over every fact that decides whether
//! a rollout can ever finish, and the reads that assemble those facts.

use clap::Args;
use serde_json::Value;

use crate::cli::release_quarantine::{
    canonical_control, compute_target, remote_read, resolve_target,
};
use crate::cli::CmdError;
use crate::deploy::{host_gates, production_runner};
use crate::release_agent::{self, release_status_uri};

use super::constants::{PHASE_UNREPORTED, VERDICT_SETTLED};
use super::quarantine::quarantine_entries;

mod candidate;
mod phase;
mod verdict;

use candidate::candidate_section;
use phase::{phase_is_rolling, phase_word};
use verdict::{diagnosis, Facts};

#[derive(Args)]
pub struct ReleaseDoctorArgs {
    pub product: String,
    /// Registry target to diagnose. Optional when the product declares
    /// exactly one.
    #[arg(long)]
    target: Option<String>,
    #[arg(long)]
    json: bool,
}

pub(super) async fn doctor(args: &ReleaseDoctorArgs) -> Result<(), CmdError> {
    let control = canonical_control().await?;
    let (target_name, policy, target_policy) =
        resolve_target(&control, &args.product, args.target.as_deref())?;
    let desired = policy.desired.as_ref();
    let desired_version = desired.map(|desired| desired.version.as_str());
    let desired_digest = desired
        .and_then(|desired| desired.artifacts.get(&target_policy.platform))
        .map(|artifact| artifact.artifact_sha256.as_str());
    let compute = compute_target(&target_name).await?;
    // The state file is read with the shared reader and parsed with the
    // agent's own parser, which checks the document's product and target
    // identity: a mistyped `--target` must fail, never diagnose one host
    // against another host's rollout.
    let state_path = release_agent::host_state_path(&target_policy.state_dir, &args.product);
    let state = match remote_read(&compute, &state_path).await?.as_deref() {
        Some(payload) => Some(
            release_agent::parse_state_document(
                payload.as_bytes(),
                &args.product,
                &target_name,
                &state_path,
            )
            .map_err(CmdError::click)?,
        ),
        None => None,
    };
    // The published status row is what `stado release status` reads. It is
    // the backstop rather than the source here: it is written FROM the state
    // file this command already reads, and the publish itself can fail
    // (every one of them answered 401 for a week when the status URI named
    // an undeclared namespace), which is precisely when a diagnosis must
    // not go blind.
    let published: Value =
        match crate::cli::storage::fetch_object(&release_status_uri(&args.product, &target_name))
            .await
        {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
    let observed_version = state
        .as_ref()
        .and_then(|state| state.active.as_ref())
        .map(|record| record.version.clone())
        .or_else(|| published["active_version"].as_str().map(str::to_string));
    let phase = state.as_ref().map_or_else(
        || {
            published["phase"]
                .as_str()
                .map_or_else(|| PHASE_UNREPORTED.to_string(), str::to_string)
        },
        |state| phase_word(state.phase),
    );
    let detail = state.as_ref().map_or_else(
        || published["detail"].as_str().unwrap_or_default().to_string(),
        |state| state.detail.clone(),
    );
    let candidate = candidate_section(
        &compute,
        state.as_ref(),
        target_policy.readiness_path.as_deref(),
    )
    .await?;
    let quarantined = quarantine_entries(state.as_ref(), desired_digest);
    // A failed gate read is a failed diagnosis, not a diagnosis with one
    // field missing. The Mac mini stopped claiming for hours on a gate
    // nothing reported; a verdict computed as if the gate were fine would
    // reproduce that incident with more confidence.
    let gates = host_gates::read_host_gates(&target_name, &production_runner())
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;

    let report = diagnosis(&Facts {
        product: &args.product,
        target: &target_name,
        desired_version,
        observed_version: observed_version.as_deref(),
        phase: &phase,
        detail: &detail,
        candidate,
        quarantined,
        gates: host_gates::gates_section(&gates),
        gate_blockers: gates.blockers.clone(),
        disk_pressure_unresolved: gates.disk_pressure_unresolved,
        // Computed from the state file this command already read, by the
        // agent's own function, so the blocker reported here and the refusal
        // the agent would apply cannot be two different rules.
        run: state.as_ref().and_then(release_agent::cause_run),
        in_flight: state
            .as_ref()
            .is_some_and(|state| state.candidate.is_some() || phase_is_rolling(state.phase)),
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let cell = |value: &Value| match value {
        Value::Null => "-".to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    println!("product           {}", args.product);
    println!("target            {target_name}");
    println!("desired           {}", cell(&report["desired_version"]));
    println!("observed          {}", cell(&report["observed_version"]));
    println!("phase             {phase}");
    if !detail.is_empty() {
        println!("detail            {detail}");
    }
    println!(
        "candidate         port={} health={} pid_alive={}",
        cell(&report["candidate"]["port"]),
        cell(&report["candidate"]["health_status"]),
        cell(&report["candidate"]["pid_alive"])
    );
    println!(
        "gates             disk_pressure_unresolved={} free_gb={} low_watermark_gb={}",
        cell(&report["gates"]["disk_pressure_unresolved"]),
        cell(&report["gates"]["free_gb"]),
        cell(&report["gates"]["low_watermark_gb"])
    );
    println!("verdict           {}", cell(&report["verdict"]));
    let blockers: Vec<String> = report["blockers"]
        .as_array()
        .map(|blockers| blockers.iter().map(cell).collect())
        .unwrap_or_default();
    println!(
        "blockers          {}",
        if blockers.is_empty() {
            "none".to_string()
        } else {
            blockers.join(", ")
        }
    );
    // The refusal, spelled out where the operator is already looking. This is
    // the line that turns "the rollout is not moving" into "the rollout is
    // being held, on purpose, for this, and here is how to overrule it".
    if let Some(run) = report["cause_run"].as_object() {
        let label = if run["held"] == Value::Bool(true) {
            "held"
        } else {
            "watching"
        };
        println!(
            "{label:<18}{} quarantine(s) share {} since {}",
            cell(&run["quarantines"]),
            cell(&run["cause"]),
            cell(&run["since"])
        );
        println!("                  {}", cell(&run["evidence"]));
        // Named, not run: the check reads the release user's vault on the host.
        // An operator reading this can run it themselves, and the agent will
        // run it before it spends the next candidate.
        if let Some(check) = run["condition_check"].as_str() {
            println!(
                "                  agent will refuse the next candidate if this still \
                 fails on the host: {check}"
            );
        }
    }
    let remedies: Vec<String> = report["remedies"]
        .as_array()
        .map(|remedies| remedies.iter().map(cell).collect())
        .unwrap_or_default();
    for (index, remedy) in remedies.iter().enumerate() {
        // One line each, all of them: the first is not necessarily the one
        // this operator needs, and a verdict that prints only the top remedy
        // hides the rest of what it just diagnosed.
        let label = if index == 0 { "remedy" } else { "" };
        println!("{label:<18}{remedy}");
    }
    let quarantined = report["quarantined"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !quarantined.is_empty() {
        // The summary, then the detail. Twenty rows of truncated stderr is not
        // a thing an operator can read, and the finding in it -- that several
        // of those rows are one cause -- was a pattern they had to spot. It is
        // a line now, and the table below still holds every row it counts.
        let summary = &report["quarantine_summary"];
        println!(
            "\nquarantined       {} on this host, {} unclassified",
            cell(&summary["total"]),
            cell(&summary["unclassified"])
        );
        match summary["dominant_cause"].as_str() {
            Some(dominant) => println!(
                "dominant cause    {dominant} ({} of {})",
                cell(&summary["dominant_count"]),
                cell(&summary["total"])
            ),
            // Said outright rather than left as an absent line: "no cause
            // dominates" and "nothing here is classified" are different
            // findings, and only the second one is about this host's evidence.
            None => println!("dominant cause    none classified"),
        }
        crate::cli::reporting::table::print(
            &["CAUSE", "COUNT", "REMEDY"],
            &summary["causes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|entry| {
                    vec![
                        cell(&entry["cause"]),
                        cell(&entry["count"]),
                        cell(&entry["remedy"]),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
        crate::cli::reporting::table::print(
            &["DIGEST", "DESIRED", "QUARANTINED AT", "CAUSE", "REASON"],
            &quarantined
                .iter()
                .map(|entry| {
                    vec![
                        cell(&entry["digest"]),
                        cell(&entry["is_desired_digest"]),
                        cell(&entry["quarantined_at"]),
                        cell(&entry["cause"]),
                        cell(&entry["reason"]),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }
    // The command that finishes the diagnosis, spelled out. In the incident
    // the state file's one sentence was the end of the trail; the log the
    // operator needed had a name nobody had written down.
    if report["verdict"] != *VERDICT_SETTLED {
        if let Some(version) = desired_version {
            println!(
                "\nnext: stado release logs {} --target {target_name} --version {version} \
                 --stream err",
                args.product
            );
        }
    }
    Ok(())
}
