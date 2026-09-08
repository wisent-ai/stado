//! This machine's own facts, read here and not taken from the product.
//!
//! Everything in this file is an independent oracle. The area used to assert
//! `stado host link` against a script named `ssh` on PATH feeding fake
//! `uname`, `id`, `stat` and `launchctl` tools, so the report could only ever
//! echo the fixture back. These reads answer the same questions from the
//! machine running the test, which is what makes a fabricated report fail:
//! the login, the console owner, the launchd domains, the real power log and
//! whether the tailnet tool exists at all.

use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;

use chrono::{DateTime, FixedOffset, SecondsFormat, Utc};

/// The transition kinds that end a sleep in `pmset -g log`, in the product's
/// own spelling (`deploy::host_link::platform::MACOS_WAKE_KINDS`).
pub const WAKE_KINDS: [&str; 2] = ["Wake", "DarkWake"];
/// The one kind that starts one.
pub const SLEEP_KINDS: [&str; 1] = ["Sleep"];

fn read(program: &str, args: &[&str]) -> String {
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("{program} runs on this machine: {error}"));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// This machine's host name in the form the registry requires of a declared
/// host name: trimmed, lower cased, no trailing dot. A registry write is
/// refused when a declared name is not normalised, so this is also the only
/// spelling the fixture may seed.
pub fn hostname() -> String {
    read("/bin/hostname", &[])
        .to_lowercase()
        .trim_end_matches('.')
        .to_string()
}

/// The leading label of that name: the slug every beacon reader keys a host
/// health object by, and the only host name `stado host publish-beacon`
/// accepts as this machine's own.
pub fn slug() -> String {
    hostname().split('.').next().unwrap_or_default().to_string()
}

/// What `uname -s` says here, which is what the beacon's collector branches
/// on.
pub fn os() -> String {
    read("/usr/bin/uname", &["-s"])
}

/// The release platform this machine really is, in the product's spelling.
pub fn release_platform() -> &'static str {
    if std::env::consts::OS == "macos" {
        "darwin-arm64"
    } else {
        "linux-amd64"
    }
}

/// The login this process runs as, and its numeric id: the two values the
/// product's own session probe reads with the same two commands.
pub fn login() -> String {
    read("/usr/bin/id", &["-un"])
}

pub fn uid() -> String {
    read("/usr/bin/id", &["-u"])
}

/// Who owns the console device. `nobody` when the read answers nothing,
/// which is the word the product's resolver substitutes for the same case.
pub fn console_owner() -> String {
    let owner = read("/usr/bin/stat", &["-f%Su", "/dev/console"]);
    if owner.is_empty() {
        "nobody".to_string()
    } else {
        owner
    }
}

/// Does launchd hold this domain here? Asked of the real launchd, with the
/// one read-only verb the product's resolver uses.
fn domain_exists(domain: &str) -> bool {
    Command::new("/bin/launchctl")
        .args(["print", domain])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// The session `stado host link` owes this machine, derived from the four
/// reads above and nothing else.
///
/// `(kind, console_owner, detail)`, in the report's own field order. The
/// detail sentences are the resolver's own, and they are reproduced here
/// rather than imported because the report is the contract: a report whose
/// sentence drifts from the machine's actual condition has to fail here.
pub fn expected_session() -> (&'static str, Option<String>, String) {
    let os = os();
    if os != "Darwin" {
        return (
            "unknown",
            None,
            format!("{os} has no console session of the kind a per-login unit needs"),
        );
    }
    let account = login();
    let uid = uid();
    let gui = format!("gui/{uid}");
    let background = format!("user/{uid}");
    let console = console_owner();
    if console == account && domain_exists(&gui) {
        return (
            "graphical",
            Some(console),
            format!(
                "{account} owns /dev/console and launchd has {gui}, so a LaunchAgent of this \
                 login loads there"
            ),
        );
    }
    if domain_exists(&background) {
        return (
            "headless",
            Some(console.clone()),
            format!(
                "/dev/console belongs to {console}, not {account}: no graphical session, so \
                 {gui} does not exist and a LaunchAgent has only the background domain \
                 {background}"
            ),
        );
    }
    (
        "headless",
        Some(console),
        format!("launchd has neither {gui} nor {background} for {account}"),
    )
}

/// The `source` values the collected block may name on this machine: this
/// platform's own pair of log-and-tailnet tools, or `unsupported` when not
/// one probe here answered.
pub fn allowed_sources() -> [&'static str; 2] {
    if os() == "Darwin" {
        ["pmset+tailscale", "unsupported"]
    } else {
        ["journalctl+tailscale", "unsupported"]
    }
}

/// The first executable named `name` on this process's `PATH`, resolved the
/// way `deploy::host_link::probes::resolve_program` resolves it. The child
/// inherits this `PATH`, so an answer here is the answer the collector got.
pub fn on_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|directory| {
        if directory.as_os_str().is_empty() {
            return None;
        }
        let candidate = directory.join(name);
        let metadata = std::fs::metadata(&candidate).ok()?;
        (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0).then_some(candidate)
    })
}

/// This machine's real power log, read once for the whole binary.
static POWER_LOG: LazyLock<String> = LazyLock::new(|| match on_path("pmset") {
    None => String::new(),
    Some(program) => {
        let output = Command::new(program)
            .args(["-g", "log"])
            .output()
            .expect("pmset -g log runs on this machine");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
});

/// The newest transition of one of `kinds` at or before `cutoff`, in the
/// instant spelling the beacon publishes, or `None` when this machine's log
/// carries none.
///
/// `cutoff` bounds the answer to what the collector could possibly have seen,
/// so this oracle does not move when the machine sleeps between the
/// product's read of the log and this one.
pub fn newest_transition(kinds: &[&str], cutoff: DateTime<Utc>) -> Option<String> {
    POWER_LOG.lines().rev().find_map(|line| {
        let mut fields = line.split_whitespace();
        let (Some(date), Some(time), Some(offset), Some(kind)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return None;
        };
        if !kinds.contains(&kind) {
            return None;
        }
        let stamp =
            DateTime::parse_from_str(&format!("{date} {time} {offset}"), "%Y-%m-%d %H:%M:%S %z")
                .ok()?;
        (stamp.with_timezone(&Utc) <= cutoff).then(|| iso(stamp))
    })
}

/// True when this machine's power log carries any sleep or wake at all. A
/// machine that has never slept would make the oracle above vacuous, and a
/// vacuous oracle has to say so rather than pass quietly.
pub fn power_log_has_transitions() -> bool {
    newest_transition(&SLEEP_KINDS, Utc::now()).is_some()
        || newest_transition(&WAKE_KINDS, Utc::now()).is_some()
}

/// One instant in the fleet's spelling: UTC, seconds, `Z`.
pub fn iso(stamp: DateTime<FixedOffset>) -> String {
    stamp
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}
