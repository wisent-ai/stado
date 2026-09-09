use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::checks::recovery::link::{reason_counts, silence_instant};
use crate::cli::host::checks::{NEWEST_SILENCES, REFUSAL_WINDOW_SECONDS};

/// The channel half of [`super::report::link`]: every declared route probed
/// in order, the one the real command chose, and who is logged in on the
/// screen.
pub(super) struct ChannelProbe {
    pub(super) connection_probes: Vec<crate::deploy::host_channel::SshConnectionProbe>,
    pub(super) connection_probe_error: Option<String>,
    pub(super) connection_degraded: bool,
    pub(super) ssh_reachable: bool,
    pub(super) ssh_error: Option<String>,
    pub(super) selected_connection: Option<String>,
    pub(super) session: crate::deploy::service::HostSession,
}

pub(super) async fn probe_channel(
    resolved: &ComputeTarget,
    runner: &crate::deploy::Runner,
) -> ChannelProbe {
    // Probe every declared route so a working preferred path does not hide a
    // broken alternate. The real command below still chooses in declaration
    // order and runs once.
    let (connection_probes, connection_probe_error) =
        match crate::deploy::host_channel::probe_ssh_connections(resolved, runner).await {
            Ok(probes) => (probes, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
    let connection_degraded =
        connection_probe_error.is_some() || connection_probes.iter().any(|probe| !probe.reachable);
    let ssh = crate::deploy::host_channel::run_program_with_connection(
        resolved,
        crate::deploy::host_ping::REMOTE_PROGRAM,
        runner,
    )
    .await;
    let (ssh_reachable, ssh_error, selected_connection) = match ssh {
        Ok((output, used)) if output.ok() => {
            let selected = match used {
                crate::deploy::host_channel::UsedConnection::Local => "local".to_string(),
                crate::deploy::host_channel::UsedConnection::Ssh(connection) => {
                    connection.name.to_string()
                }
            };
            (true, None, Some(selected))
        }
        Ok((output, _)) => (
            false,
            Some(crate::deploy::host_channel::last_error_line(
                &output,
                "ssh failed",
            )),
            None,
        ),
        Err(exc) => (false, Some(exc.to_string()), None),
    };

    // The one fact neither surface could state, and the reason the mini takes
    // no work: whether anybody is logged in on its screen. Asked only of a
    // host that just answered, so an unreachable box costs one connect attempt
    // here rather than two, and answered by the same resolver
    // `stado service restart` uses, so a diagnostic and a repair cannot
    // disagree about the session underneath them.
    let session = match &ssh_error {
        None => crate::deploy::service::read_session(resolved, runner).await,
        Some(detail) => crate::deploy::service::HostSession::unknown(format!(
            "this host did not answer, so nobody could ask it whether anyone is logged in on its \
             screen: {detail}"
        )),
    };

    ChannelProbe {
        connection_probes,
        connection_probe_error,
        connection_degraded,
        ssh_reachable,
        ssh_error,
        selected_connection,
        session,
    }
}

/// The beacon half of [`super::report::link`]: the aged signal and the `link`
/// block the host published inside its beacon.
pub(super) async fn probe_beacon(
    store: &crate::queue::JobStorage,
    resolved: &ComputeTarget,
) -> Result<(crate::deploy::host_ping::BeaconSignal, Option<Value>), CmdError> {
    // The beacon half, aged by the one rule `host ping` ages every beacon in
    // this fleet with, and the `link` block the host published inside it.
    let now = chrono::Utc::now();
    let (signal, published) =
        match crate::monitor::host_health::load_host_health(store, &resolved.name).await {
            Ok(report) => {
                let published = crate::deploy::host_link::BeaconLink::from_beacon(&report.beacon)
                    .map(serde_json::to_value)
                    .transpose()?;
                (
                    crate::deploy::host_ping::grade_beacon(&report, now),
                    published,
                )
            }
            Err(exc) => (
                crate::deploy::host_ping::BeaconSignal::unreadable(exc.to_string()),
                None,
            ),
        };

    Ok((signal, published))
}

/// The silence half of [`super::report::link`]: the observation this command
/// IS, the records it produced, and the reader refusals inside the window.
pub(super) async fn collect_silences(
    store: &crate::queue::JobStorage,
    resolved: &ComputeTarget,
    signal: &crate::deploy::host_ping::BeaconSignal,
    blockers: &mut Vec<String>,
) -> (
    Vec<crate::monitor::host_silence::SilenceRecord>,
    crate::monitor::host_silence::RefusalSummary,
    bool,
) {
    // Looking at a beacon IS the observation the silence record is made of,
    // and [`crate::monitor::host_silence::observe_beacon_age`] is the one
    // entry point for the transition: whichever component notices the
    // threshold crossing writes it, and three observers of one gap produce one
    // record carrying three names. An operator running this command during an
    // outage is exactly that — the observer who noticed — and on 2026-08-19
    // nothing recorded what they saw. The instant is the beacon's own, recovered
    // with the same parser that aged it: a silence's `started_at` is when the
    // host was last heard from, and deriving it from the rounded age would
    // misdate every record by up to a second.
    let newest_beacon_at = signal
        .reported_at
        .as_deref()
        .and_then(crate::deploy::host_ping::parse_timestamp);
    if let Err(exc) = crate::monitor::host_silence::observe_beacon_age(
        store,
        &resolved.name,
        newest_beacon_at,
        crate::monitor::host_silence::READER_CLI,
        signal.error.as_deref(),
    )
    .await
    {
        blockers.push(exc.to_string());
    }

    // A store that will not answer for the silences is reported as a blocker
    // and never as a failed command. Refusing to print the half that was read
    // is the exact behaviour this command exists to end.
    let silences =
        match crate::monitor::host_silence::recent_silences(store, &resolved.name, NEWEST_SILENCES)
            .await
        {
            Ok(records) => records,
            Err(exc) => {
                blockers.push(exc.to_string());
                Vec::new()
            }
        };
    let refusals = match crate::monitor::host_silence::refusal_summary(
        store,
        &resolved.name,
        REFUSAL_WINDOW_SECONDS,
    )
    .await
    {
        Ok(summary) => summary,
        Err(exc) => {
            blockers.push(exc.to_string());
            crate::monitor::host_silence::RefusalSummary::empty(REFUSAL_WINDOW_SECONDS)
        }
    };
    let refused = refusals.count > usize::MIN;
    if refused {
        blockers.push(format!(
            "readers refused {} time(s) in the last {}s: {}",
            refusals.count,
            refusals.window_seconds,
            reason_counts(&refusals),
        ));
    }
    // The open record's own first reader error, verbatim: it is what a reader
    // wrote down at the moment the host stopped answering, and once the host is
    // back it is the only account of the gap that exists.
    if let Some(open) = silences.iter().find(|record| record.ended_at.is_none()) {
        let mut sentence = format!(
            "a silence opened at {} is still open",
            silence_instant(open.started_at)
        );
        if let Some(detail) = &open.first_reader_error {
            sentence.push_str(&format!("; first reader error: {detail}"));
        }
        blockers.push(sentence);
    }

    (silences, refusals, refused)
}
