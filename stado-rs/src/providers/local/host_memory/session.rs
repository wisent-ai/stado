//! The one repair that reaches outside software Stado declared: terminating
//! a named process of the logged-in graphical session.
//!
//! It exists because that is what actually held the memory on
//! charless-mac-mini on 2026-09-06 — WindowManager at 718 MB, Safari with
//! eight WebKit content processes, Messages spinning at 90% CPU — and no
//! amount of restarting Stado's own units would have returned it. It is also
//! the one repair that can lose a person's unsaved work, so it is the one
//! repair that needs two declarations: the registry must name the repair AND
//! the repair must carry `allow_graphical_session: true`. Declared without
//! the flag, this module reports exactly which processes it would end and how
//! much each holds, and ends none of them.

use std::process::Command;

use super::report::RepairReport;
use super::schema::MemoryRepairPolicy;

/// One matching process of the graphical session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProcess {
    pub pid: i32,
    pub uid: u32,
    /// Resident set size in kibibytes, as `ps` reports it.
    pub rss_kb: i64,
    /// Seconds since the process started.
    pub age_seconds: i64,
    pub command: String,
}

/// Read the process table once.
fn process_table() -> Option<String> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,uid=,rss=,etime=,comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// `[[dd-]hh:]mm:ss` as seconds.
pub fn parse_etime(value: &str) -> Option<i64> {
    let minute = 60_i64;
    let hour = minute * minute;
    let day = hour * 24;
    let (days, clock) = match value.split_once('-') {
        Some((days, clock)) => (days.parse::<i64>().ok()?, clock),
        None => (0, value),
    };
    let mut parts: Vec<i64> = Vec::new();
    for part in clock.split(':') {
        parts.push(part.parse::<i64>().ok()?);
    }
    let clock_seconds = match parts.as_slice() {
        [minutes, seconds] => minutes * minute + seconds,
        [hours, minutes, seconds] => hours * hour + minutes * minute + seconds,
        _ => return None,
    };
    Some(days * day + clock_seconds)
}

/// Parse one `ps` row.
pub fn parse_row(line: &str) -> Option<SessionProcess> {
    let mut columns = line.split_whitespace();
    let pid = columns.next()?.parse::<i32>().ok()?;
    let uid = columns.next()?.parse::<u32>().ok()?;
    let rss_kb = columns.next()?.parse::<i64>().ok()?;
    let age_seconds = parse_etime(columns.next()?)?;
    let command = columns.collect::<Vec<&str>>().join(" ");
    if command.is_empty() {
        return None;
    }
    Some(SessionProcess {
        pid,
        uid,
        rss_kb,
        age_seconds,
        command,
    })
}

/// The declared name a command path matches, if any.
///
/// Matched on the executable's own last path component, so a declaration
/// says `Safari` and not the bundle path launchd happened to start it from.
pub fn declared_match<'a>(command: &str, declared: &'a [String]) -> Option<&'a String> {
    let leaf = command.rsplit('/').next().unwrap_or(command);
    declared.iter().find(|name| name.as_str() == leaf)
}

/// Every declared process currently running, largest first.
pub fn matching_processes(declared: &[String]) -> Vec<SessionProcess> {
    let Some(table) = process_table() else {
        return Vec::new();
    };
    let mut found: Vec<SessionProcess> = table
        .lines()
        .filter_map(parse_row)
        .filter(|process| declared_match(&process.command, declared).is_some())
        .collect();
    found.sort_by_key(|process| std::cmp::Reverse(process.rss_kb));
    found
}

/// Terminate declared graphical-session processes, when and only when the
/// declaration carries `allow_graphical_session`.
pub fn terminate_declared(
    policy: &MemoryRepairPolicy,
    enforce: bool,
    budget: &mut i64,
    log_fn: &mut dyn FnMut(&str),
) -> (RepairReport, Vec<String>) {
    let mut report = RepairReport {
        subjects: policy.processes.clone(),
        ..RepairReport::default()
    };
    let mut errors = Vec::new();
    let note = |report: &mut RepairReport, reason: &str| {
        *report.skipped.entry(reason.to_string()).or_insert(0) += 1;
    };
    for process in matching_processes(&policy.processes) {
        report.examined += 1;
        if let Some(minimum) = policy.min_age_seconds {
            if process.age_seconds < minimum {
                note(&mut report, "younger_than_declared_age");
                continue;
            }
        }
        report.eligible += 1;
        if !enforce {
            note(&mut report, "report_only");
            continue;
        }
        if !policy.allow_graphical_session {
            note(&mut report, "graphical_session_not_authorized");
            continue;
        }
        if *budget <= 0 {
            note(&mut report, "budget_reached");
            continue;
        }
        *budget -= 1;
        log_fn(&format!(
            "memory: ending declared session process {} (pid {}, {} KiB resident)",
            process.command, process.pid, process.rss_kb
        ));
        match nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(process.pid),
            nix::sys::signal::Signal::SIGTERM,
        ) {
            Ok(()) => report.repaired += 1,
            Err(error) => {
                note(&mut report, "signal_refused");
                errors.push(format!(
                    "{} (pid {}): {error}",
                    process.command, process.pid
                ));
            }
        }
    }
    (report, errors)
}
