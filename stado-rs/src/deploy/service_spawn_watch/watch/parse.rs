//! Turning the marker stream the remote loop prints back into a report.

use crate::deploy::host_channel;
use crate::deploy::service_spawn_watch::{Ancestor, Arrival, Baseline, ProcessRow, WatchReport};

/// Turn the marker stream into a report. Pure. A marker whose fields do not
/// read is kept in `malformed`, never read as zero or dropped.
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
        let read = match host_channel::marker_fields(line).as_slice() {
            ["STADO_WATCH_UNSUPPORTED", system] => {
                report.unsupported = Some((*system).trim().to_string());
                true
            }
            ["STADO_WATCH_BASELINE", _pid, row] => match ProcessRow::parse(row) {
                Some(row) => {
                    report.baseline.push(Baseline { row });
                    true
                }
                None => false,
            },
            ["STADO_WATCH_ARRIVAL", sequence, _pid, after, row] => {
                match (
                    sequence.trim().parse(),
                    after.trim().parse(),
                    ProcessRow::parse(row),
                ) {
                    (Ok(sequence), Ok(after_seconds), Some(row)) => {
                        report.arrivals.push(Arrival {
                            sequence,
                            after_seconds,
                            row,
                            ancestry: Vec::new(),
                        });
                        true
                    }
                    _ => false,
                }
            }
            ["STADO_WATCH_ANCESTOR", sequence, depth, _pid, alive, row] => {
                match (
                    sequence.trim().parse::<u32>(),
                    depth.trim().parse(),
                    ProcessRow::parse(row),
                ) {
                    (Ok(sequence), Ok(depth), Some(row)) => {
                        let ancestor = Ancestor {
                            depth,
                            alive: alive.trim() == "yes",
                            row,
                        };
                        match report
                            .arrivals
                            .iter_mut()
                            .find(|arrival| arrival.sequence == sequence)
                        {
                            Some(arrival) => {
                                arrival.ancestry.push(ancestor);
                                true
                            }
                            None => false,
                        }
                    }
                    _ => false,
                }
            }
            ["STADO_WATCH_DONE", samples, elapsed] => {
                match (samples.trim().parse(), elapsed.trim().parse()) {
                    (Ok(samples), Ok(elapsed)) => {
                        report.samples = Some(samples);
                        report.elapsed_seconds = Some(elapsed);
                        true
                    }
                    _ => false,
                }
            }
            _ => true,
        };
        if !read {
            report.malformed.push(line.to_string());
        }
    }
    report
}
