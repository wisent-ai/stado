//! `stado host link` against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! into a test. The registry, the beacon, the silence records and the refusal
//! records all live and die inside the temp dir; nothing here touches the
//! operator's real registry, vault, or hosts.
//!
//! What is being defended: a connectivity gap used to leave no trace in this
//! product. control-host answered no ping and no ssh from 18:29 to 18:35
//! UTC on 2026-08-19 and came back on `direct 10.0.0.253:41641`; the only
//! evidence was two ping packets an operator sent by hand, and the refusals it
//! caused went to a log file nobody reads. These tests pin the three verdicts
//! this command answers with, the sentences it names them in, and the silence
//! record it leaves behind on disk.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde_json::Value;

/// The silence threshold is pinned rather than defaulted: the fleet default is
/// 300 seconds, and a test that read the operator's environment for it would
/// change verdict on the machine it ran on.
const THRESHOLD_SECONDS: &str = "300";

fn stado(storage: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
    cmd.args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        // A set-but-missing STADO_CONFIG disables config-file discovery.
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env("STADO_SILENCE_THRESHOLD_SECONDS", THRESHOLD_SECONDS)
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .env_remove("WC_PROFILES_DIR");
    cmd.output().expect("stado binary runs")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The `--json` document, parsed.
fn document(out: &Output) -> Value {
    serde_json::from_str(&stdout(out)).expect("host link --json prints one JSON document")
}

const REGISTRY: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "h1",
            "kind": "local",
            "ssh": "u@127.0.0.1",
            "release_platform": "darwin-arm64",
            "hostnames": ["h1.local"],
            "slots": 1
        }
    ],
    "coordinators": []
}"#;

/// A temp storage root carrying exactly this registry document.
fn storage_with_registry() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("registry.json"), REGISTRY).unwrap();
    dir
}

/// Write one blob at the exact path the contract gives it.
///
/// The paths are spelled out here rather than taken from the product's own
/// helpers: `host_health/<slug>.json`, `host_silence/<host>/<started_at>.json`
/// and `reader_refusals/<host>/<at>.json` ARE the contract, and a test that
/// asked the code under test where to write would pin nothing.
fn write_blob(storage: &Path, path: &str, body: &str) {
    let full = storage.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, body).unwrap();
}

/// Blob-key spelling of an instant: compact, microsecond, UTC, so that
/// lexicographic order over the keys is chronological order.
fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y%m%dT%H%M%S%.6fZ").to_string()
}

/// Beacon-document spelling of an instant, to the second, as the beacon
/// writers emit it.
fn beacon_time(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Every silence record on disk for `host`, oldest key first.
fn silences_on_disk(storage: &Path, host: &str) -> Vec<Value> {
    let dir = storage.join("host_silence").join(host);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{} is unreadable: {err}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            serde_json::from_str(&std::fs::read_to_string(path).unwrap())
                .expect("a silence record stays JSON")
        })
        .collect()
}

/// The `link` block a host publishes when its collector could read everything.
fn link_block(collected_at: &str) -> String {
    format!(
        r#"{{
            "collected_at": "{collected_at}",
            "path_kind": "direct",
            "endpoint": "10.0.0.253:41641",
            "last_sleep_at": "2026-08-19T18:28:55Z",
            "last_wake_at": "2026-08-19T18:35:02Z",
            "interface_changes": [
                {{"at": "2026-08-19T18:29:01Z", "detail": "en0 link down"}},
                {{"at": "2026-08-19T18:35:04Z", "detail": "en0 link up"}}
            ],
            "source": "pmset+tailscale"
        }}"#
    )
}

#[test]
fn host_link_reports_the_published_link_and_closes_the_open_silence() {
    let dir = storage_with_registry();
    let storage = dir.path();
    let now = Utc::now();
    let reported_at = beacon_time(now - TimeDelta::seconds(30));

    write_blob(
        storage,
        "host_health/h1.json",
        &format!(
            r#"{{"host": "h1", "reported_at": "{reported_at}", "link": {}}}"#,
            link_block(&reported_at)
        ),
    );
    // A gap somebody else opened half an hour ago and nobody closed. The
    // fresher beacon is what ends it, and this command is what notices.
    let opened = now - TimeDelta::seconds(1800);
    write_blob(
        storage,
        &format!("host_silence/h1/{}.json", stamp(opened)),
        &format!(
            r#"{{"host": "h1", "started_at": "{}", "ended_at": null, "duration_seconds": null,
                  "first_reader_error": "service directory cache is stale",
                  "observed_by": ["resolver"]}}"#,
            beacon_time(opened)
        ),
    );

    let out = stado(storage, &["host", "link", "h1", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "a fresh beacon with nothing refused is healthy: {}",
        stderr(&out)
    );
    let report = document(&out);
    assert_eq!(report["host"], "h1");
    assert_eq!(report["verdict"], "healthy");
    // The published block, field for field: this is what the operator read over
    // ssh by hand on 2026-08-19 and could not read here.
    assert_eq!(report["path_kind"], "direct");
    assert_eq!(report["endpoint"], "10.0.0.253:41641");
    assert_eq!(report["last_sleep_at"], "2026-08-19T18:28:55Z");
    assert_eq!(report["last_wake_at"], "2026-08-19T18:35:02Z");
    assert_eq!(
        report["interface_changes"],
        serde_json::json!([
            {"at": "2026-08-19T18:29:01Z", "detail": "en0 link down"},
            {"at": "2026-08-19T18:35:04Z", "detail": "en0 link up"}
        ])
    );
    assert_eq!(report["reader_refusals"]["count"], 0);
    assert_eq!(report["reader_refusals"]["window_seconds"], 3600);
    assert_eq!(report["reader_refusals"]["reasons"], serde_json::json!({}));
    assert!(
        report["beacon_age_seconds"].as_i64().unwrap() < 300,
        "the seeded beacon is inside the threshold, got: {}",
        report["beacon_age_seconds"]
    );

    // The state that matters is on disk: the gap is closed at the instant the
    // host published again, not at the instant somebody looked, so the duration
    // is the outage and not the polling interval.
    let records = silences_on_disk(storage, "h1");
    assert_eq!(records.len(), 1, "one gap, closed, not a second record");
    assert_eq!(records[0]["ended_at"], reported_at.as_str());
    assert_eq!(records[0]["duration_seconds"], 1770);
    assert_eq!(
        records[0]["observed_by"],
        serde_json::json!(["resolver", "cli"]),
        "the reader that closed the gap is named beside the one that opened it"
    );
    assert_eq!(
        records[0]["first_reader_error"],
        "service directory cache is stale",
        "closing a gap never rewrites what the first reader said about it"
    );
    // The closed record is the one the document carried.
    assert_eq!(report["silences"].as_array().unwrap().len(), 1);
    assert_eq!(report["silences"][0]["duration_seconds"], 1770);

    // The report form prints the same facts, in the shape `host gates` uses.
    let out = stado(storage, &["host", "link", "h1"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    for line in [
        "host:     h1",
        "verdict:  healthy",
        "path:     direct via 10.0.0.253:41641",
        "sleep:    last slept 2026-08-19T18:28:55Z, last woke 2026-08-19T18:35:02Z",
        "changes:  2 recorded",
        "          2026-08-19T18:29:01Z en0 link down",
        "refusals: none in the last 3600s",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
}

#[test]
fn host_link_calls_a_stale_host_silent_and_records_the_gap() {
    let dir = storage_with_registry();
    let storage = dir.path();
    let now = Utc::now();
    // Past the 300-second threshold, and publishing the older beacon that
    // carries no link block at all.
    let reported_at = beacon_time(now - TimeDelta::seconds(900));
    write_blob(
        storage,
        "host_health/h1.json",
        &format!(r#"{{"host": "h1", "reported_at": "{reported_at}"}}"#),
    );

    let out = stado(storage, &["host", "link", "h1", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a verdict that is not healthy exits 1: {}",
        stderr(&out)
    );
    let report = document(&out);
    assert_eq!(report["verdict"], "silent");
    assert_eq!(report["ssh_reachable"], false);
    // A missing link block is nulls and a named blocker, never a fabricated
    // path: "we do not know how this host is reachable" is the answer that
    // sends an operator to look.
    assert_eq!(report["path_kind"], "unknown");
    assert_eq!(report["endpoint"], Value::Null);
    assert_eq!(report["last_sleep_at"], Value::Null);
    assert_eq!(report["last_wake_at"], Value::Null);
    assert_eq!(report["interface_changes"], serde_json::json!([]));

    let blockers: Vec<&str> = report["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|blocker| blocker.as_str().unwrap())
        .collect();
    assert!(
        blockers.iter().any(|blocker| blocker
            .ends_with("s old, past the 300s silence threshold")
            && blocker.starts_with("this host's newest beacon is")),
        "the staleness blocker names the age and the threshold, got: {blockers:?}"
    );
    assert!(
        blockers.contains(
            &"this host's beacon carries no link block, so its path, its sleep and wake \
              times and its interface changes are unknown here"
        ),
        "the missing link block is named in the component's own words, got: {blockers:?}"
    );
    assert!(
        stderr(&out).contains("h1 link verdict is silent, with"),
        "the refusal names the verdict, got: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("blocker(s) named in the report above"),
        "got: {}",
        stderr(&out)
    );

    // The gap this command noticed is now on disk, keyed by the instant the
    // host was last heard from — which is the whole point: on 2026-08-19 the
    // six minutes control-host spent unreachable were recorded nowhere.
    let records = silences_on_disk(storage, "h1");
    assert_eq!(records.len(), 1, "one open record, got: {records:?}");
    assert_eq!(records[0]["host"], "h1");
    assert_eq!(records[0]["started_at"], reported_at.as_str());
    assert_eq!(records[0]["ended_at"], Value::Null);
    assert_eq!(records[0]["duration_seconds"], Value::Null);
    assert_eq!(records[0]["observed_by"], serde_json::json!(["cli"]));

    // Looking twice records one gap with one observer, not two records.
    let out = stado(storage, &["host", "link", "h1", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(silences_on_disk(storage, "h1").len(), 1);
}

#[test]
fn host_link_calls_a_host_degraded_when_readers_refused() {
    let dir = storage_with_registry();
    let storage = dir.path();
    let now = Utc::now();
    let reported_at = beacon_time(now - TimeDelta::seconds(20));
    write_blob(
        storage,
        "host_health/h1.json",
        &format!(r#"{{"host": "h1", "reported_at": "{reported_at}"}}"#),
    );
    // The two refusals the 2026-08-19 gap actually produced, in the words the
    // components wrote them in.
    for (offset, reason, detail) in [
        (
            120,
            "authority_unreachable",
            "registry authority exited: ssh connect Operation timed out",
        ),
        (
            60,
            "directory_cache_stale",
            "service directory cache is stale",
        ),
    ] {
        let at = now - TimeDelta::seconds(offset);
        write_blob(
            storage,
            &format!("reader_refusals/h1/{}.json", stamp(at)),
            &format!(
                r#"{{"host": "h1", "at": "{}", "reader": "resolver",
                      "reason": "{reason}", "detail": "{detail}"}}"#,
                beacon_time(at)
            ),
        );
    }

    let out = stado(storage, &["host", "link", "h1", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "a host readers refused about is not healthy: {}",
        stderr(&out)
    );
    let report = document(&out);
    // The beacon is fresh, so this is not silence: it is a host nobody could
    // read while it was perfectly well.
    assert_eq!(report["verdict"], "degraded");
    assert_eq!(
        report["reader_refusals"],
        serde_json::json!({
            "window_seconds": 3600,
            "count": 2,
            "reasons": {"authority_unreachable": 1, "directory_cache_stale": 1}
        })
    );
    let blockers: Vec<&str> = report["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|blocker| blocker.as_str().unwrap())
        .collect();
    assert!(
        blockers.contains(
            &"readers refused 2 time(s) in the last 3600s: authority_unreachable=1, \
              directory_cache_stale=1"
        ),
        "the refusal blocker counts each reason token, got: {blockers:?}"
    );
    assert!(
        stderr(&out).contains("h1 link verdict is degraded, with"),
        "got: {}",
        stderr(&out)
    );
    // A fresh beacon leaves no silence behind, so there is nothing to record.
    assert!(!storage.join("host_silence/h1").exists());

    let out = stado(storage, &["host", "link", "h1"]);
    assert!(
        stdout(&out).contains(
            "refusals: 2 in the last 3600s: authority_unreachable=1, directory_cache_stale=1"
        ),
        "the report form counts the same refusals, got:\n{}",
        stdout(&out)
    );
}

#[test]
fn host_link_refuses_a_target_the_registry_does_not_carry() {
    let dir = storage_with_registry();
    let storage = dir.path();

    let out = stado(storage, &["host", "link", "nope", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("target 'nope' is not in the canonical registry"),
        "got: {}",
        stderr(&out)
    );
    // A refused target produces no document at all: half a report about a host
    // that does not exist is worse than none.
    assert!(stdout(&out).trim().is_empty(), "got: {}", stdout(&out));
    assert!(!storage.join("host_silence").exists());
}

// ---------------------------------------------------------------------------
// The session behind the link
// ---------------------------------------------------------------------------
//
// The operator's question was "where do I see whether anyone is logged in on
// that host", and the answer was nowhere. These tests pin the answer.
//
// The host is a fake, and it is a fake in one place: `ssh`. The product pipes
// its fixed remote script to `ssh` on stdin, so a script named `ssh` on PATH
// receives that script byte for byte and runs it against the fake `uname` /
// `id` / `stat` / `launchctl` in `host-bin`. Those tools are NOT on the
// caller's PATH — only the far side of that hop sees them.
//
// The login is `charles` and the uid is 501 because they are control-host's
// own, so every sentence asserted below is the sentence the live host printed
// on 2026-08-19 rather than a rephrasing of it.

/// The remote script arrives on stdin. Only the four tools the session read
/// touches are rewritten to bare names, so `host-bin` means something; every
/// other absolute path stays absolute and resolves to the real tool.
///
/// The reachability half of `host link` sends a fixed program and no stdin at
/// all, so `sed` reads EOF, `bash` runs nothing, and the fake host answers ssh
/// — which is the state that makes the session read run at all.
const FAKE_SSH: &str = r#"#!/bin/sh
PATH="$STADO_FAKE_HOST_BIN:$PATH"; export PATH
/usr/bin/sed \
  -e "s#/usr/bin/uname#uname#g" \
  -e "s#/usr/bin/id#id#g" \
  -e "s#/usr/bin/stat#stat#g" \
  -e "s#/bin/launchctl#launchctl#g" \
  | /bin/bash -s
"#;

/// An ssh that never connects, the way a box that has dropped off the network
/// answers. Its last stderr line is what the operator has to be shown.
const DEAD_SSH: &str = r#"#!/bin/sh
echo "ssh: connect to host 10.9.9.11 port 22: Operation timed out" >&2
exit 255
"#;

const FAKE_UNAME: &str = "#!/bin/sh\ncat \"$STADO_FAKE_STATE/os\"\n";

/// The login on the far side: the mini's own account and uid.
const FAKE_ID: &str = r#"#!/bin/sh
case "${1:-}" in
  -un) echo charles ;;
  *) echo 501 ;;
esac
"#;

/// The one read that says whether anybody is logged in graphically:
/// `stat -f%Su /dev/console` is the console's owner — the login user while a
/// session exists, root at the login window.
const FAKE_STAT: &str = r#"#!/bin/sh
case "${1:-}" in
  -f%Su) cat "$STADO_FAKE_STATE/console" ;;
  *) exit 1 ;;
esac
"#;

/// launchd, reduced to the one verb the session read uses: does this domain
/// exist. `state/gui` is the graphical domain, and `state/no_user_domain`
/// takes the background one away as well.
const FAKE_LAUNCHCTL: &str = r#"#!/bin/sh
S="$STADO_FAKE_STATE"
[ "${1:-}" = print ] || exit 0
case "${2:-}" in
  gui/*)
    [ -f "$S/gui" ] || { echo "Could not find domain for ${2}" >&2; exit 113; }
    ;;
  user/*)
    if [ -f "$S/no_user_domain" ]; then
      echo "Could not find domain for ${2}" >&2
      exit 113
    fi
    ;;
esac
exit 0
"#;

/// The unit control-host declares as a LaunchAgent and cannot start.
const AGENT: &str = "com.wisent.compute.service.stado-agent-mini";
/// Where that declaration puts it.
const AGENT_PATH: &str =
    "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist";
/// The daemon spelling of the same job — the only domain the host can load.
const DAEMON_PATH: &str =
    "/Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist";

/// A temp root carrying a registry, a fake host behind a fake ssh, and a HOME.
struct FakeHost {
    dir: tempfile::TempDir,
}

impl FakeHost {
    /// Seeded in the state control-host is in: macOS, nobody at the
    /// screen so `/dev/console` is root's, no `gui/501`, and the agent
    /// declared as a per-login unit.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for sub in ["ssh-bin", "host-bin", "state", "storage", "home"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        let host_bin = root.join("host-bin");
        for (dir, name, body) in [
            (root.join("ssh-bin"), "ssh", FAKE_SSH),
            (host_bin.clone(), "uname", FAKE_UNAME),
            (host_bin.clone(), "id", FAKE_ID),
            (host_bin.clone(), "stat", FAKE_STAT),
            (host_bin, "launchctl", FAKE_LAUNCHCTL),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        // The owner-only key file the channel's identity seam accepts in place
        // of the broker. Nothing reads it: the ssh it is handed to is a script.
        let key = root.join("state/ssh-key");
        std::fs::write(&key, "-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();

        let host = Self { dir };
        host.state("os", "Darwin\n");
        host.state("console", "root\n");
        host.declare(Some(AGENT_PATH));
        host.publish_fresh_beacon();
        host
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn state(&self, name: &str, content: &str) {
        std::fs::write(self.root().join("state").join(name), content).unwrap();
    }

    /// Somebody holds the screen: the console is this login's, and launchd has
    /// the graphical domain that comes with it.
    fn graphical_session(&self) {
        self.state("console", "charles\n");
        self.state("gui", "");
    }

    /// The box stopped answering between the operator asking and the command
    /// running.
    fn does_not_answer(&self) {
        let path = self.root().join("ssh-bin/ssh");
        std::fs::write(&path, DEAD_SSH).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The registry this fake host is declared in. `always-on` is the word the
    /// declaration check reads, and `hostnames` deliberately does not name this
    /// machine so the channel really goes through the fake `ssh`.
    fn declare(&self, unit_path: Option<&str>) {
        let services = match unit_path {
            None => String::new(),
            Some(path) => format!(
                "{{\"name\": \"{AGENT}\", \"unit\": \"\", \"label\": \"{AGENT}\", \
                 \"path\": \"{path}\", \"kind\": \"launchd\", \
                 \"managed_since\": \"2026-08-01T00:00:00Z\"}}"
            ),
        };
        std::fs::write(
            self.root().join("storage/registry.json"),
            format!(
                "{{\"schema_version\": 2, \"targets\": [{{\
                 \"name\": \"fake-mini\", \"kind\": \"local\", \
                 \"ssh\": \"charles@10.9.9.11\", \"role\": \"always-on\", \
                 \"release_platform\": \"darwin-arm64\", \
                 \"hostnames\": [\"fake-mini.local\"], \"slots\": 1, \
                 \"services\": [{services}]}}], \"coordinators\": []}}"
            ),
        )
        .unwrap();
    }

    /// A beacon fresh enough to keep the verdict out of the way. What these
    /// tests are about is the session, not the silence.
    fn publish_fresh_beacon(&self) {
        let reported_at = beacon_time(Utc::now() - TimeDelta::seconds(30));
        write_blob(
            &self.root().join("storage"),
            "host_health/fake-mini.json",
            &format!(
                r#"{{"host": "fake-mini", "reported_at": "{reported_at}", "link": {}}}"#,
                link_block(&reported_at)
            ),
        );
    }

    fn stado(&self, args: &[&str]) -> Output {
        let root = self.root();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(args)
            .env_clear()
            .env("HOME", root.join("home"))
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                    root.join("ssh-bin").display()
                ),
            )
            .env("STADO_FAKE_HOST_BIN", root.join("host-bin"))
            .env("STADO_FAKE_STATE", root.join("state"))
            .env("STADO_HOST_SSH_KEY_FILE", root.join("state/ssh-key"))
            .env("STADO_SILENCE_THRESHOLD_SECONDS", THRESHOLD_SECONDS)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", root.join("storage"))
            // A set-but-missing STADO_CONFIG disables config-file discovery.
            .env("STADO_CONFIG", root.join("storage/no-such-config.json"));
        cmd.output().expect("stado binary runs")
    }
}

/// The blocker a headless host carrying this declaration owes its operator,
/// spelled the way the command spells it — the plain sentence first, the
/// privileged command that closes it second.
fn agent_blocker() -> String {
    format!(
        "nobody is logged in on the screen here, and {AGENT} is registered as a user service, so \
         this machine cannot start it; install it as a machine service with one privileged \
         command on the host: sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel \
         {AGENT_PATH} {DAEMON_PATH} && /usr/bin/plutil -insert UserName -string charles \
         {DAEMON_PATH}'"
    )
}

/// True when the document names the session as a reason work cannot start.
fn names_the_session(report: &Value) -> bool {
    report["blockers"]
        .as_array()
        .expect("blockers is an array")
        .iter()
        .any(|entry| {
            entry
                .as_str()
                .is_some_and(|line| line.contains("logged in on the screen here"))
        })
}

/// control-host's whole condition in one document: nobody at the screen,
/// a unit only a screen can start, and the command that moves it.
#[test]
fn a_headless_host_declaring_a_user_service_names_it_as_a_blocker() {
    let host = FakeHost::new();

    let out = host.stado(&["host", "link", "fake-mini", "--json"]);
    let report = document(&out);
    assert_eq!(report["session"]["kind"], "headless");
    assert_eq!(report["session"]["console_owner"], "root");
    // The resolver's own sentence, unabridged. This is what `stado service
    // restart --host control-host --json` printed under
    // `launchd_domain.reason` on 2026-08-19, and it is what a reader who
    // wants the next command needs.
    assert_eq!(
        report["session"]["detail"],
        "/dev/console belongs to root, not charles: no graphical session, so gui/501 does not \
         exist and a LaunchAgent has only the background domain user/501"
    );
    assert!(
        report["blockers"]
            .as_array()
            .unwrap()
            .contains(&Value::String(agent_blocker())),
        "the blocker is missing from {:#}",
        report["blockers"]
    );
    // The verdict rules did not learn about the session: this host's beacon is
    // fresh and nothing refused, so it is healthy and the command exits 0 with
    // the blocker named in the document.
    assert_eq!(
        report["verdict"], "healthy",
        "a headless host is not by itself unhealthy: {}",
        stderr(&out)
    );
    assert_eq!(out.status.code(), Some(0));

    // The report form: the operator's words on the line they read first, the
    // domain vocabulary underneath it and never the other way round.
    let out = host.stado(&["host", "link", "fake-mini"]);
    let text = stdout(&out);
    for line in [
        "session:  nobody is logged in on the screen here",
        "          /dev/console belongs to root, not charles: no graphical session, so gui/501 \
         does not exist and a LaunchAgent has only the background domain user/501",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
}

/// The same declaration on a host somebody is logged into is not a blocker.
/// A LaunchAgent is exactly what a graphical session is for, and a finding
/// that fired here would be a finding an operator learns to ignore.
#[test]
fn a_graphical_host_with_the_same_declaration_names_no_blocker() {
    let host = FakeHost::new();
    host.graphical_session();

    let out = host.stado(&["host", "link", "fake-mini", "--json"]);
    let report = document(&out);
    assert_eq!(report["session"]["kind"], "graphical");
    assert_eq!(report["session"]["console_owner"], "charles");
    assert_eq!(
        report["session"]["detail"],
        "charles owns /dev/console and launchd has gui/501, so a LaunchAgent of this login loads \
         there"
    );
    assert_eq!(
        report["blockers"],
        serde_json::json!([]),
        "nothing is wrong with this host: {}",
        stderr(&out)
    );
    assert_eq!(out.status.code(), Some(0));

    let text = stdout(&host.stado(&["host", "link", "fake-mini"]));
    assert!(
        text.contains("session:  charles is logged in on the screen here"),
        "missing the session line in:\n{text}"
    );
}

/// Headless on its own is not the fault. The same host with its unit declared
/// where the machine can load it has nothing blocking it, and still reports
/// the session — the fact stays visible after the repair.
#[test]
fn a_headless_host_declaring_only_a_machine_service_names_no_blocker() {
    let host = FakeHost::new();
    host.declare(Some(DAEMON_PATH));

    let report = document(&host.stado(&["host", "link", "fake-mini", "--json"]));
    assert_eq!(report["session"]["kind"], "headless");
    assert_eq!(report["session"]["console_owner"], "root");
    assert_eq!(
        report["blockers"],
        serde_json::json!([]),
        "a headless host is not by itself unhealthy"
    );
}

/// launchd with no per-login domain at all is still a host nobody is logged
/// in on, and still a host that cannot start a per-login unit. Reading this
/// state as "unknown" would silence the blocker on a host that is certainly
/// blocked.
#[test]
fn a_host_whose_launchd_has_no_per_login_domain_is_still_headless() {
    let host = FakeHost::new();
    host.state("no_user_domain", "");

    let report = document(&host.stado(&["host", "link", "fake-mini", "--json"]));
    assert_eq!(report["session"]["kind"], "headless");
    assert_eq!(
        report["session"]["detail"],
        "launchd has neither gui/501 nor user/501 for charles"
    );
    assert!(
        report["blockers"]
            .as_array()
            .unwrap()
            .contains(&Value::String(agent_blocker())),
        "the blocker is missing from {:#}",
        report["blockers"]
    );
}

/// A host that is not a mac has no console session of this kind to read, and
/// the honest answer is that the question does not apply — not a guess in
/// either direction, and never a blocker.
#[test]
fn a_host_that_is_not_a_mac_reports_an_unknown_session() {
    let host = FakeHost::new();
    host.state("os", "Linux\n");

    let report = document(&host.stado(&["host", "link", "fake-mini", "--json"]));
    assert_eq!(report["session"]["kind"], "unknown");
    assert_eq!(report["session"]["console_owner"], Value::Null);
    assert_eq!(
        report["session"]["detail"],
        "Linux has no console session of the kind a per-login unit needs"
    );
    assert!(!names_the_session(&report));
}

/// The rule the whole probe is built under: a session nobody could read never
/// costs the operator a fact they already had. The host that does not answer
/// still publishes its path, its sleep times and its verdict, and the session
/// is `unknown` carrying ssh's own last line rather than an error that eats
/// the document.
#[test]
fn a_host_that_does_not_answer_reports_an_unknown_session_and_stays_readable() {
    let host = FakeHost::new();
    host.does_not_answer();

    let out = host.stado(&["host", "link", "fake-mini", "--json"]);
    let report = document(&out);
    assert_eq!(report["ssh_reachable"], false);
    assert_eq!(report["session"]["kind"], "unknown");
    assert_eq!(report["session"]["console_owner"], Value::Null);
    assert_eq!(
        report["session"]["detail"],
        "this host did not answer, so nobody could ask it whether anyone is logged in on its \
         screen: ssh: connect to host 10.9.9.11 port 22: Operation timed out"
    );
    assert!(
        !names_the_session(&report),
        "a blocker must never be invented from a session nobody read: {:#}",
        report["blockers"]
    );
    // Everything the command could still read is still here.
    assert_eq!(report["path_kind"], "direct");
    assert_eq!(report["endpoint"], "10.0.0.253:41641");
    assert_eq!(report["verdict"], "healthy");
    assert_eq!(out.status.code(), Some(0));
}
