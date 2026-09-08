//! Turning the marker stream the remote loop prints back into a report.

use crate::deploy::host_channel;
use crate::deploy::service_spawn_watch::{Ancestor, Arrival, Baseline, ProcessRow, WatchReport};

/// Turn the marker stream into a report. Pure — covered by unit tests.
pub fn parse_watch(
    host: &str,
    matched: &str,
    seconds: u64,
    interval_ms: u64,
    stdout: &str,
) -> WatchReport {
    let mut report = WatchReport {
        host: host.to_string(),
        matched: matched.to_string(),
        seconds,
        interval_ms,
        ..WatchReport::default()
    };
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_WATCH_UNSUPPORTED", system] => {
                report.unsupported = Some((*system).trim().to_string());
            }
            ["STADO_WATCH_BASELINE", _pid, row] => {
                if let Some(row) = ProcessRow::parse(row) {
                    report.baseline.push(Baseline { row });
                }
            }
            ["STADO_WATCH_ARRIVAL", sequence, _pid, after, row] => {
                if let Some(row) = ProcessRow::parse(row) {
                    report.arrivals.push(Arrival {
                        sequence: sequence.trim().parse().unwrap_or_default(),
                        after_seconds: after.trim().parse().unwrap_or_default(),
                        row,
                        ancestry: Vec::new(),
                    });
                }
            }
            ["STADO_WATCH_ANCESTOR", sequence, depth, _pid, alive, row] => {
                let sequence: u32 = sequence.trim().parse().unwrap_or_default();
                let Some(row) = ProcessRow::parse(row) else {
                    continue;
                };
                let ancestor = Ancestor {
                    depth: depth.trim().parse().unwrap_or_default(),
                    alive: alive.trim() == "yes",
                    row,
                };
                if let Some(arrival) = report
                    .arrivals
                    .iter_mut()
                    .find(|arrival| arrival.sequence == sequence)
                {
                    arrival.ancestry.push(ancestor);
                }
            }
            ["STADO_WATCH_DONE", samples, elapsed] => {
                report.samples = samples.trim().parse().unwrap_or_default();
                report.elapsed_seconds = elapsed.trim().parse().unwrap_or_default();
            }
            _ => {}
        }
    }
    report
}
