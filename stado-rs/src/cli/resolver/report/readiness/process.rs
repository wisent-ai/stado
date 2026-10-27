//! What the serving resolver process is doing, as the operating system
//! counts it: the processor time it has taken and how long it has run, read
//! with POSIX `ps` (`ps -o time= -o etime= -p PID`, both fields as `ps`
//! prints them). A process whose time grows as fast as its age holds a whole
//! core, which is what a status that only says `serving` never showed. The
//! reading is a measurement or the reason it was not taken, never a guess.

use serde_json::{json, Value};

/// The processor time field `ps` is asked for.
const TIME: &str = "time=";
/// The age field `ps` is asked for.
const ELAPSED: &str = "etime=";

/// The processor time and age of `pid` on this host, or why they were not
/// read. `local` is false when the resolver runs on another host, whose
/// processes this machine cannot see.
pub(super) fn reading(pid: Option<u32>, local: bool) -> Value {
    let Some(pid) = pid else {
        return json!({"read": false, "why": "the resolver publishes no process id"});
    };
    if !local {
        return json!({"read": false, "why": "the resolver runs on another host; ask that host with `stado host inventory`"});
    }
    let process = pid.to_string();
    let mut command = std::process::Command::new("ps");
    command.args(["-o", TIME, "-o", ELAPSED, "-p", &process]);
    let shown = format!("ps -o {TIME} -o {ELAPSED} -p {pid}");
    let output = match crate::wait::output(&mut command) {
        Ok(output) => output,
        Err(error) => {
            return json!({"read": false, "why": format!("{shown} could not run: {error}")})
        }
    };
    if !output.status.success() {
        return json!({"read": false, "why": format!(
            "{shown} ended with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )});
    }
    let printed = String::from_utf8_lossy(&output.stdout);
    let mut words = printed.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some(cpu), Some(elapsed), None) => json!({
            "read": true,
            "pid": pid,
            "cpuTime": cpu,
            "elapsed": elapsed,
            "source": shown,
        }),
        _ => {
            json!({"read": false, "why": format!("{shown} printed {printed:?}, not a time and an age")})
        }
    }
}
