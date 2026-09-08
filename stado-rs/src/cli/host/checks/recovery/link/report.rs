use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::checks::health::api::host_health_beacon_unit;
use crate::cli::host::checks::health::beacon_store;
use crate::cli::host::checks::health::units::{collect_unit_log, host_health_publisher_diagnosis};
use crate::cli::host::checks::probes::print_json;
use crate::cli::host::checks::recovery::link::link_outcome;
use crate::cli::host::checks::{
    HOST_HEALTH_LOG_LINES, LINK_DEGRADED, LINK_HEALTHY, LINK_SILENT, PATH_KIND_UNKNOWN,
};

/// `stado host link TARGET [--json]` — why this host went quiet, in one
/// payload.
///
/// The incident: between 18:29 and 18:35 UTC on 2026-08-19 control-host
/// answered no ping and no ssh, then came back on `direct 10.0.0.253:41641`.
/// Six minutes of a host being unreachable left no trace anywhere in this
/// product. The only evidence was two ping packets an operator happened to
/// send, and the reader-side refusals it caused — "service directory cache is
/// stale", "registry authority exited: ssh connect Operation timed out" — went
/// to `~/.stado/logs/stado-resolver.err` and nowhere a person would look. This
/// command is the trace: the host's own account of its path and its sleep and
/// wake times, the silences recorded against it, and what refused because of
/// them.
///
/// Everything here is read. Opening and closing a silence belongs to the
/// observer path in [`crate::monitor::host_silence`]; a diagnostic that
/// recorded a silence every time an operator looked would make the count it
/// prints a function of how often it was run.
///
/// The exit status follows `verdict`, the way `host gates`' follows `claiming`,
/// so `stado host link mini && ...` is a usable sentence and a silent host
/// cannot be mistaken for a healthy one by a script that reads only status
/// codes.
pub async fn link(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let store = beacon_store().await?;
    let mut blockers: Vec<String> = Vec::new();

    // The registry through the last-known-good cache, not the authority alone.
    // This is the command an operator runs while the control plane is the thing
    // that is sick: on 2026-08-19 every host command died on the same refused
    // ssh the operator was trying to diagnose, which is a diagnostic that dies
    // with its subject.
    let (registry, notice) = crate::targets::fetch_registry_or_last_good()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if let Some(sentence) = notice {
        // On stderr so `--json` stays exactly one document on stdout, and in
        // the blockers so the cache's age reaches whoever reads the document
        // instead of the terminal.
        eprintln!("{sentence}");
        blockers.push(sentence);
    }
    let resolved = crate::deploy::host_channel::resolve_target(&registry, target)
        .map_err(|exc| CmdError::click(exc.to_string()))?;

    let super::probe::ChannelProbe {
        connection_probes,
        connection_probe_error,
        connection_degraded,
        ssh_reachable,
        ssh_error,
        selected_connection,
        session,
    } = super::probe::probe_channel(resolved, &runner).await;

    let (signal, published) = super::probe::probe_beacon(&store, resolved).await?;

    let from_link = |key: &str| {
        published
            .as_ref()
            .and_then(|block| block.get(key).cloned())
            .unwrap_or(Value::Null)
    };

    let threshold = crate::monitor::host_silence::silence_threshold_seconds();
    // No age at all — no beacon object, an unparseable one, an unreadable store
    // — counts as past the threshold. An absent beacon is the strongest form of
    // "nothing has been heard from this host", not an exemption from it.
    let stale = signal.age_seconds.is_none_or(|age| age > threshold);
    // A reachable host with a stale beacon is a publisher failure, not a
    // network mystery. Read the managed publisher's own log here so `link`
    // carries the cause an operator previously had to discover with a second
    // command.
    let beacon_publisher = if stale && ssh_reachable {
        let publisher_unit = host_health_beacon_unit(resolved);
        match collect_unit_log(resolved, publisher_unit, HOST_HEALTH_LOG_LINES, &runner).await {
            Ok(report) => Some(host_health_publisher_diagnosis(&report)),
            Err(error) => Some(json!({
                "unit": publisher_unit,
                "code": "diagnostic_unavailable",
                "detail": error.to_string(),
                "repairable": false,
            })),
        }
    } else {
        None
    };
    if let Some(detail) = beacon_publisher
        .as_ref()
        .and_then(|publisher| publisher.get("detail"))
        .and_then(Value::as_str)
    {
        blockers.push(detail.to_string());
    }

    if let Some(detail) = &signal.error {
        blockers.push(detail.clone());
    }
    if let (true, Some(age)) = (stale, signal.age_seconds) {
        blockers.push(format!(
            "this host's newest beacon is {age}s old, past the {threshold}s silence threshold"
        ));
    }
    if let Some(detail) = &ssh_error {
        blockers.push(detail.clone());
    }
    if let Some(detail) = &connection_probe_error {
        blockers.push(format!("connection path probes failed: {detail}"));
    }
    for probe in connection_probes.iter().filter(|probe| !probe.reachable) {
        blockers.push(format!(
            "{} connection path {} did not answer: {}",
            probe.name,
            probe.destination,
            probe.error.as_deref().unwrap_or("SSH probe failed")
        ));
    }
    if published.is_none() {
        blockers.push(
            "this host's beacon carries no link block, so its path, its sleep and wake \
             times and its interface changes are unknown here"
                .to_string(),
        );
    }

    // A headless host is not a fault, and the verdict rules do not learn about
    // this one. A headless host carrying a unit that only a logged-in screen
    // can start IS the fault, and it is the fault that stops work:
    // control-host has three of them and a job that has waited days for
    // the capacity they would publish. The declaration half is
    // `deploy::service::misdeclared_domains` rather than a second opinion
    // about it; what is added here is the half that had to be read from the
    // host. One blocker per unit, because each needs its own command run.
    if session.is_headless() {
        for misdeclared in crate::deploy::service::misdeclared_domains(resolved) {
            blockers.push(format!(
                "nobody is logged in on the screen here, and {} is registered as a user service, \
                 so this machine cannot start it; install it as a machine service with one \
                 privileged command on the host: {}",
                misdeclared.unit,
                misdeclared.install_command()
            ));
        }
    }

    let (silences, refusals, refused) =
        super::probe::collect_silences(&store, resolved, &signal, &mut blockers).await;

    let verdict = if stale {
        // A box that answers ssh while nothing has heard from its agent is not
        // silent: it is running and not reporting, which is a different repair
        // and the exact state that ran for five days in July.
        if ssh_reachable {
            LINK_DEGRADED
        } else {
            LINK_SILENT
        }
    } else if refused || connection_degraded {
        LINK_DEGRADED
    } else {
        LINK_HEALTHY
    };

    let path_kind = match from_link("path_kind") {
        Value::Null => Value::String(PATH_KIND_UNKNOWN.to_string()),
        kind => kind,
    };
    let changes = match from_link("interface_changes") {
        Value::Array(changes) => changes,
        _ => Vec::new(),
    };
    let recorded = silences
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<Value>, _>>()?;
    let report = json!({
        "host": resolved.name,
        "beacon_age_seconds": signal.age_seconds,
        "ssh_reachable": ssh_reachable,
        "selected_connection": &selected_connection,
        "connection_paths": &connection_probes,
        "connection_probe_error": &connection_probe_error,
        "session": session.to_json(),
        "beacon_publisher": &beacon_publisher,
        "path_kind": path_kind,
        "endpoint": from_link("endpoint"),
        "last_sleep_at": from_link("last_sleep_at"),
        "last_wake_at": from_link("last_wake_at"),
        "interface_changes": changes,
        "silences": recorded,
        "reader_refusals": {
            "window_seconds": refusals.window_seconds,
            "count": refusals.count,
            "reasons": refusals.reasons,
        },
        "verdict": verdict,
        "blockers": blockers,
    });
    if json {
        print_json(&report);
        return link_outcome(&resolved.name, verdict, blockers.len());
    }

    super::render::render(
        resolved,
        verdict,
        blockers,
        &signal,
        beacon_publisher,
        ssh_error,
        connection_probe_error,
        connection_probes,
        selected_connection,
        session,
        published,
        path_kind,
        changes,
        refused,
        refusals,
        silences,
    )
}
