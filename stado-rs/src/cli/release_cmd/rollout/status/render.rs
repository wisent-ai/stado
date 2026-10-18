//! Render the retained pipeline diagnostics without discarding delivery failures.
use serde_json::Value;

/// A duration an operator reads at a glance: `47s`, `18m34s`, `2h05m`. A
/// negative span (a clock that ran behind) or one too long to split prints as
/// the seconds it is, never as no time at all.
fn human_seconds(seconds: i64) -> String {
    use chrono::TimeDelta;
    let Some(span) = TimeDelta::try_seconds(seconds).filter(|_| !seconds.is_negative()) else {
        return format!("{seconds}s");
    };
    let hours = span.num_hours();
    let minutes = (span - TimeDelta::hours(hours)).num_minutes();
    let rest = (span - TimeDelta::hours(hours) - TimeDelta::minutes(minutes)).num_seconds();
    if hours.is_positive() {
        return format!("{hours}h{minutes:02}m");
    }
    if minutes.is_positive() {
        return format!("{minutes}m{rest:02}s");
    }
    format!("{rest}s")
}

pub(super) fn print_runs(runs: &[Value]) {
    if !runs.is_empty() {
        println!("--- pipeline runs (newest first):");
    }
    for run in runs {
        println!(
            "run {} {} {} {} {} {}{}",
            recorded(run, "run_id"),
            recorded(run, "product"),
            recorded(run, "version"),
            recorded(run, "channel"),
            recorded(run, "state"),
            recorded(run, "updated_at"),
            // The build the run consumes, so `stado build status` can be
            // asked about the jobs without reading the run JSON.
            run["build_id"]
                .as_str()
                .map(|build| format!(" build={build}"))
                .unwrap_or_default(),
        );
        // Each platform on its own line: the run-level state alone reads
        // as a promise, while "linux-amd64 submitted job=<id> [running]" is a
        // fact an operator can go and watch.
        for (platform, record) in run["platforms"].as_object().into_iter().flatten() {
            let mut line = format!(
                "  {platform} {} job={}",
                recorded(record, "state"),
                recorded(record, "job_id"),
            );
            if let Some(job_state) = record["job_state"].as_str() {
                line.push_str(&format!(" [{job_state}]"));
            }
            // What it cost, from the job's own clock. A release that is
            // getting slower is invisible in a register that prints only
            // states, and "the build takes hours" was diagnosed by hand.
            if let Some(seconds) = record["build_seconds"].as_i64() {
                let running = matches!(record["job_state"].as_str(), Some("running" | "queue"));
                line.push_str(&format!(
                    " {} {}",
                    if running { "running for" } else { "took" },
                    human_seconds(seconds)
                ));
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
                print_failure("    ", failure);
            }
            if let Some(error) = record["job_read_error"].as_str() {
                println!("    job read error: {error}");
            }
        }
        for (name, record) in run["deliveries"].as_object().into_iter().flatten() {
            println!(
                "  delivery {name} state={} platform={} required={} job_id={}",
                record["state"], record["platform"], record["required"], record["job_id"]
            );
            println!("    output prefix: {}", record["output_prefix"]);
            if let Some(digest) = record["receipt_sha256"].as_str() {
                println!("    accepted receipt SHA-256: {digest}");
            }
            if let Some(failure) = record["failure"].as_str() {
                print_failure("    ", failure);
            }
        }
        if let Some(failure) = run["failure"].as_str() {
            print_failure("  ", failure);
        }
    }
}

/// A field of a run or platform record as the register holds it, or which
/// field it left out: a dash read the same for every one of them.
fn recorded(record: &Value, field: &str) -> String {
    match record[field].as_str() {
        Some(value) => value.to_string(),
        None => format!("(no {field})"),
    }
}

/// A failure as the run recorded it, whole: its first line names the failing
/// job, and the lines after it are the step names and the output the worker
/// wrote, which is the diagnosis. Printing the first line alone ended the
/// register at "the job's last output:" with nothing under it and sent the
/// reader to `stado build status` for what this register already held.
fn print_failure(indent: &str, failure: &str) {
    let mut lines = failure.lines();
    if let Some(first) = lines.next() {
        println!("{indent}failure: {first}");
    }
    for line in lines {
        println!("{indent}  {line}");
    }
}
