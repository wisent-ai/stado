//! Table and JSON renderings of the runner capability's read-only reports.
//!
//! They live beside the dispatcher rather than inside it because the dispatcher
//! has to stay small enough to add a leaf to, and because a rendering is the
//! one part of a command that carries no policy: every decision these make is
//! about columns.

use serde_json::Value;

use super::{click, connected, print_json, text};
use crate::cli::CmdError;

pub(super) fn render_profiles(json: bool) -> Result<(), CmdError> {
    let declaration = crate::deploy::host_precheck_runner::runner_declaration()
        .map_err(|error| click(error, json))?;
    if json {
        print_json(&serde_json::to_value(declaration)?);
        return Ok(());
    }
    println!("PROFILE\tSLUG\tRUNNER GROUP\tLABELS");
    for profile in &declaration.profiles {
        println!(
            "{}\t{}\t{}\t{}",
            profile.name,
            profile.slug,
            profile.github_runner_group,
            profile.labels.join(",")
        );
    }
    Ok(())
}

pub(super) fn render_status_set(report: &Value, json: bool) {
    if json {
        print_json(report);
        return;
    }
    for profile in report
        .get("profiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!(
            "{}: installed {}; scope {}; listener {}; host job slot {}",
            text(profile.get("profile")),
            profile
                .get("installed")
                .and_then(Value::as_bool)
                .map_or("unknown", |installed| if installed { "yes" } else { "no" }),
            text(profile.get("runner_scope")),
            connected(profile),
            text(profile.get("host_job_slot")),
        );
    }
}

pub(super) fn render_fleet(report: &Value, json: bool) {
    if json {
        print_json(report);
        return;
    }
    println!("TARGET\tPROFILE\tINSTALLED\tSCOPE\tLISTENER\tHOST JOB SLOT");
    for host in report
        .get("hosts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for profile in host
            .get("profiles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                text(host.get("target")),
                text(profile.get("profile")),
                profile
                    .get("installed")
                    .and_then(Value::as_bool)
                    .map_or("unknown", |installed| if installed { "yes" } else { "no" }),
                text(profile.get("runner_scope")),
                connected(profile),
                text(profile.get("host_job_slot")),
            );
        }
    }
}
