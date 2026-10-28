//! What `launchctl print` says of a loaded job: its environment block, its
//! state and its pid.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

static ENVIRONMENT_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*environment = \{\s*$").expect("static"));
static BLOCK_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\}\s*$").expect("static"));
static ENTRY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?P<key>[^=\s]+)\s+=>\s+(?P<value>.*?)\s*$").expect("static")
});
static STATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*state = (?P<state>.*?)\s*$").expect("static"));
static PID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*pid = (?P<pid>[0-9]+)\s*$").expect("static"));

/// `launchctl print` output: the job's environment block, its state and pid.
pub(super) fn from_launchctl(text: &str) -> (BTreeMap<String, String>, String, String) {
    let (mut environment, mut state, mut pid) = (BTreeMap::new(), None, None);
    let mut inside = false;
    for line in text.lines() {
        if ENVIRONMENT_OPEN.is_match(line) {
            inside = true;
            continue;
        }
        if inside {
            if BLOCK_CLOSE.is_match(line) {
                inside = false;
            } else if let Some(found) = ENTRY.captures(line) {
                environment.insert(found["key"].to_string(), found["value"].to_string());
            }
            continue;
        }
        if let Some(found) = STATE.captures(line) {
            state.get_or_insert_with(|| found["state"].to_string());
        }
        if let Some(found) = PID.captures(line) {
            pid.get_or_insert_with(|| found["pid"].to_string());
        }
    }
    // launchctl prints no state line for a job it does not hold and no pid
    // line for one that is not running; each is said as such.
    (
        environment,
        match state {
            Some(state) => state,
            None => "(launchctl printed no state)".into(),
        },
        match pid {
            Some(pid) => pid,
            None => "(no pid: not running)".into(),
        },
    )
}
