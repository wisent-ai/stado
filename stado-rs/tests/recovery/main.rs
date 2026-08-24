//! Recovery of a unit the approved channel is not privileged to bootstrap.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! into a test, and HOME is inside the temp dir so nothing the remote program
//! expands can reach the operator's own `~/.stado`.
//!
//! The host is a fake, and it is a fake in one specific place: `ssh`. The
//! product pipes its fixed remote program to `ssh` on stdin, so a script named
//! `ssh` on PATH receives the real program, byte for byte, and runs it against
//! the fake `launchctl` / `plutil` / `PlistBuddy` / `pgrep` / `ps` / `kill` in
//! `host-bin`. Those tools are NOT on the caller's PATH — only the far side of
//! that hop sees them — because `deploy/host_channel.rs` runs a target whose
//! `hostnames` match this machine locally instead of over ssh, and a fake
//! `hostname` visible to the product would silently turn every one of these
//! tests into a run against the developer's own launchd.
//!
//! What the fake host models is the situation the 2026-08-19 object-API outage
//! turned on: a LaunchDaemon in `/Library/LaunchDaemons`, whose job belongs to
//! root, whose process runs as the approved unprivileged user because the plist
//! carries `UserName`, and which launchd keeps alive. So the fake `kill` is the
//! interesting one: it ends the process and, when the unit declares KeepAlive,
//! puts a replacement pid in the table — which is the entire bet this repair
//! makes.
//!
//! Every sentence asserted here was copied from a hand run against this
//! harness, never guessed.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

/// A temp root carrying a registry, a fake host, and a HOME.
struct Harness {
    dir: tempfile::TempDir,
}

/// The remote program arrives on stdin. Rewrite only the tools these tests
/// fake, plus the one directory tree launchd owns, and run it.
///
/// `kill` is rewritten to an absolute path and the rest to bare names on
/// purpose: `kill` is a bash builtin, so PATH cannot shadow it, while every
/// other tool has to resolve through PATH for `host-bin` to mean anything.
const FAKE_SSH: &str = r#"#!/bin/sh
PATH="$STADO_FAKE_HOST_BIN:$PATH"; export PATH
/usr/bin/sed \
  -e "s#/Library/LaunchDaemons#$STADO_FAKE_STATE/LaunchDaemons#g" \
  -e "s#/bin/launchctl#launchctl#g" \
  -e "s#/usr/bin/plutil#plutil#g" \
  -e "s#/usr/libexec/PlistBuddy#PlistBuddy#g" \
  -e "s#/usr/bin/pgrep#pgrep#g" \
  -e "s#/bin/ps#ps#g" \
  -e "s#/bin/sleep#sleep#g" \
  -e "s#/usr/bin/sudo#sudo#g" \
  -e "s#/bin/hostname#hostname#g" \
  -e "s#/usr/bin/uname#uname#g" \
  -e "s#/usr/bin/id#id#g" \
  -e "s#/bin/kill#$STADO_FAKE_HOST_BIN/kill#g" \
  -e "s#/usr/bin/stat#stat#g" \
  | /bin/bash -s
"#;

const FAKE_UNAME: &str = "#!/bin/sh\necho Darwin\n";

const FAKE_HOSTNAME: &str = "#!/bin/sh\ncat \"$STADO_FAKE_STATE/hostname\"\n";

/// `/dev/console` belongs to root: nobody is logged in graphically on this
/// fake host, which is the state control-host is in and the reason its
/// agent domain is the `user/501` fallback. Faked rather than left to the real
/// `stat` so a pass never reads the developer's own console.
const FAKE_STAT: &str = r#"#!/bin/sh
case "${1:-}" in
  -f%Su) echo root ;;
  *) exit 1 ;;
esac
"#;

/// The approved login: an ordinary account, never root.
const FAKE_ID: &str = r#"#!/bin/sh
case "${1:-}" in
  -un) echo approved ;;
  *) echo 501 ;;
esac
"#;

/// What `sudo -n` does on the always-on host: nothing, loudly.
const FAKE_SUDO: &str = r#"#!/bin/sh
echo "sudo: a password is required" >&2
exit 1
"#;

/// `print` answers for the per-login domain and refuses the system domain,
/// which is what an unprivileged `launchctl print system/<label>` does.
const FAKE_LAUNCHCTL: &str = r#"#!/bin/sh
case "${1:-}" in
  print)
    case "${2:-}" in
      gui/*|user/*) exit 0 ;;
      *) echo "Could not find service \"${2:-}\"" >&2; exit 113 ;;
    esac ;;
esac
exit 0
"#;

/// `KeepAlive` comes out of `state/keepalive`: a scalar word, `DICT` for the
/// conditional spelling that has no raw form, or `MISSING`. Unit environment
/// values come out of `state/env/<NAME>`.
const FAKE_PLUTIL: &str = r#"#!/bin/sh
if [ "${1:-}" = "-lint" ]; then
  [ -f "$STADO_FAKE_STATE/plist_unreadable" ] && exit 1
  exit 0
fi
key="${2:-}"; fmt="${3:-}"
case "$key" in
  KeepAlive)
    raw=$(cat "$STADO_FAKE_STATE/keepalive" 2>/dev/null || echo MISSING)
    case "$raw" in
      MISSING) exit 1 ;;
      DICT) if [ "$fmt" = xml1 ]; then echo "<dict><key>SuccessfulExit</key><false/></dict>"; exit 0; fi; exit 1 ;;
      *) if [ "$fmt" = raw ]; then printf '%s\n' "$raw"; else printf '<%s/>\n' "$raw"; fi; exit 0 ;;
    esac ;;
  EnvironmentVariables.*)
    file="$STADO_FAKE_STATE/env/${key#EnvironmentVariables.}"
    [ -f "$file" ] || exit 1
    cat "$file" ;;
  *) exit 1 ;;
esac
"#;

/// The unit declares one program and no arguments, in both the spellings the
/// product reads: the whole `ProgramArguments` array, which is how a unit's
/// pids are scoped now, and the first element.
const FAKE_PLISTBUDDY: &str = r#"#!/bin/sh
case "${2:-}" in
  "Print :ProgramArguments")
    echo "Array {"
    /usr/bin/sed 's/^/    /' "$STADO_FAKE_STATE/program"
    echo "}"
    ;;
  "Print :ProgramArguments:0") cat "$STADO_FAKE_STATE/program" ;;
  *) exit 1 ;;
esac
"#;

/// The fake process table is `state/pids`, one `pid user` per line; every
/// process in it runs the unit's one program.
const FAKE_PGREP: &str =
    "#!/bin/sh\n/usr/bin/awk '{print $1}' \"$STADO_FAKE_STATE/pids\" 2>/dev/null\n";

const FAKE_PS: &str = r#"#!/bin/sh
want=""
field=""
prev=""
for arg in "$@"; do
  if [ "$prev" = "-p" ]; then want="$arg"; fi
  case "$arg" in user=|comm=|command=) field="$arg" ;; esac
  prev="$arg"
done
/usr/bin/awk -v pid="$want" '$1 == pid {found = 1} END {exit !found}' "$STADO_FAKE_STATE/pids" 2>/dev/null || exit 1
case "$field" in
  comm=|command=) cat "$STADO_FAKE_STATE/program" ;;
  *) /usr/bin/awk -v pid="$want" '$1 == pid {print $2}' "$STADO_FAKE_STATE/pids" 2>/dev/null ;;
esac
"#;

/// Waiting is the one thing a test must not do for real.
const FAKE_SLEEP: &str = "#!/bin/sh\nexit 0\n";

/// launchd, in eleven lines: TERM ends the process, and a job whose plist
/// declares KeepAlive gets a replacement pid.
const FAKE_KILL: &str = r#"#!/bin/sh
pid="$2"
table="$STADO_FAKE_STATE/pids"
owner=$(/usr/bin/awk -v p="$pid" '$1 == p {print $2}' "$table")
[ -n "$owner" ] || exit 1
/usr/bin/awk -v p="$pid" '$1 != p' "$table" > "$table.next"
if [ -f "$STADO_FAKE_STATE/respawn" ]; then
  echo "$((pid + 1000)) $owner" >> "$table.next"
fi
mv "$table.next" "$table"
"#;

/// `fake-mini` declares the two units this fleet keeps as system daemons.
/// `fake-agent-mini` declares no services at all, so the recovery pass falls
/// back to `host_recovery::MANAGED_AGENTS` and its per-login agent path.
const REGISTRY: &str = r#"{
  "schema_version": 2,
  "targets": [
    {
      "name": "fake-mini",
      "kind": "local",
      "ssh": "approved@10.9.9.9",
      "release_platform": "darwin-arm64",
      "hostnames": ["fake-mini.local"],
      "slots": 1,
      "services": [
        {
          "name": "com.wisent.always-on.stado-object-api",
          "unit": "",
          "label": "com.wisent.always-on.stado-object-api",
          "path": "/Library/LaunchDaemons/com.wisent.always-on.stado-object-api.plist",
          "kind": "launchd",
          "managed_since": "2026-08-01T00:00:00Z"
        },
        {
          "name": "com.wisent.host-health-beacon",
          "unit": "",
          "label": "com.wisent.host-health-beacon",
          "path": "/Library/LaunchDaemons/com.wisent.host-health-beacon.plist",
          "kind": "launchd",
          "managed_since": "2026-08-07T00:00:00Z"
        }
      ]
    },
    {
      "name": "fake-agent-mini",
      "kind": "local",
      "ssh": "approved@10.9.9.10",
      "release_platform": "darwin-arm64",
      "hostnames": ["fake-agent-mini.local"],
      "slots": 1
    }
  ],
  "coordinators": []
}"#;

const OBJECT_API: &str = "com.wisent.always-on.stado-object-api";
const BEACON: &str = "com.wisent.host-health-beacon";
const DAEMON_ROOT: &str = "/Library/LaunchDaemons";

fn tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl Harness {
    /// A temp root with the fake host seeded for the healthy case: a system
    /// daemon that declares KeepAlive and runs as the approved user.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for child in [
            "ssh-bin",
            "host-bin",
            "state",
            "state/env",
            "storage",
            "home",
        ] {
            std::fs::create_dir_all(root.join(child)).unwrap();
        }
        tool(&root.join("ssh-bin"), "ssh", FAKE_SSH);
        let host_bin = root.join("host-bin");
        tool(&host_bin, "uname", FAKE_UNAME);
        tool(&host_bin, "hostname", FAKE_HOSTNAME);
        tool(&host_bin, "id", FAKE_ID);
        tool(&host_bin, "sudo", FAKE_SUDO);
        tool(&host_bin, "stat", FAKE_STAT);
        tool(&host_bin, "launchctl", FAKE_LAUNCHCTL);
        tool(&host_bin, "plutil", FAKE_PLUTIL);
        tool(&host_bin, "PlistBuddy", FAKE_PLISTBUDDY);
        tool(&host_bin, "pgrep", FAKE_PGREP);
        tool(&host_bin, "ps", FAKE_PS);
        tool(&host_bin, "sleep", FAKE_SLEEP);
        tool(&host_bin, "kill", FAKE_KILL);

        std::fs::write(root.join("storage/registry.json"), REGISTRY).unwrap();

        // `deploy/ssh_key.rs` materializes a target-scoped key for every
        // host-channel command. STADO_HOST_SSH_KEY_FILE is the product's own
        // owner-only override for a broker that cannot answer, which is
        // exactly this test's situation.
        let key = root.join("state/ssh-key");
        std::fs::write(&key, "-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();

        let harness = Self { dir };
        harness.host("fake-mini");
        harness.state("keepalive", "true\n");
        harness.state("program", "/Users/approved/.stado/bin/stado\n");
        harness.state("pids", "471 approved\n");
        harness.state("respawn", "");
        harness.declare_daemon(OBJECT_API);
        harness.declare_daemon(BEACON);
        for (name, value) in [
            ("STADO_HOST_HEALTH_API_URL", "http://127.0.0.1:8765"),
            (
                "STADO_HOST_HEALTH_SKARBIEC_URL",
                "https://skarbiec.wisent.com",
            ),
            (
                "STADO_HOST_HEALTH_SKARBIEC_CONSUMER",
                "stado-host-health-beacon",
            ),
            (
                "STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE",
                "/Users/approved/.stado/host-health-beacon-skarbiec-token",
            ),
            ("STADO_BIN", "/Users/approved/.stado/bin/stado"),
        ] {
            harness.state(&format!("env/{name}"), value);
        }
        harness
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn state(&self, name: &str, content: &str) {
        std::fs::write(self.root().join("state").join(name), content).unwrap();
    }

    fn read_state(&self, name: &str) -> String {
        std::fs::read_to_string(self.root().join("state").join(name)).unwrap()
    }

    /// What the fake host answers `hostname -s` with; the recovery program
    /// refuses a host whose identity does not match the target.
    fn host(&self, name: &str) {
        self.state("hostname", &format!("{name}\n"));
    }

    /// Put a unit file where the fake host's `/Library/LaunchDaemons` is.
    fn declare_daemon(&self, label: &str) {
        let dir = self.root().join("state/LaunchDaemons");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{label}.plist")),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\
                 <key>Label</key><string>{label}</string>\n\
                 <key>UserName</key><string>approved</string>\n\
                 <key>KeepAlive</key><true/>\n</dict></plist>\n"
            ),
        )
        .unwrap();
    }

    fn remove_daemon(&self, label: &str) {
        std::fs::remove_file(
            self.root()
                .join("state/LaunchDaemons")
                .join(format!("{label}.plist")),
        )
        .unwrap();
    }

    /// Put a unit file in the per-login LaunchAgents directory of the fake
    /// HOME, which is where `MANAGED_AGENTS` looks when nothing is declared.
    fn declare_agent(&self, label: &str) {
        let dir = self.root().join("home/Library/LaunchAgents");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{label}.plist")),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\
                 <key>Label</key><string>{label}</string>\n</dict></plist>\n"
            ),
        )
        .unwrap();
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
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", root.join("storage"))
            // A set-but-missing STADO_CONFIG disables config-file discovery.
            .env("STADO_CONFIG", root.join("storage/no-such-config.json"));
        cmd.output().expect("stado binary runs")
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The `host recover` document, parsed out of what the command printed.
fn document(out: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(out)).expect("host recover prints one JSON document")
}

/// `service restart` of a system LaunchDaemon whose process the approved user
/// owns and whose job launchd keeps alive: the process is replaced, the report
/// says so, and it says why that is a restart.
#[test]
fn service_restart_ends_the_owned_process_of_a_keepalive_daemon() {
    let host = Harness::new();

    let out = host.stado(&["service", "restart", OBJECT_API, "--host", "fake-mini"]);
    assert!(
        out.status.success(),
        "restart failed: {}{}",
        stdout(&out),
        stderr(&out)
    );

    // The state that matters is the process table: pid 471 is gone and
    // launchd has put a different pid in its place under the same account.
    assert_eq!(
        host.read_state("pids"),
        "1471 approved\n",
        "the owned process must be replaced, not merely signalled"
    );

    let printed = stdout(&out);
    assert!(
        printed.contains("restarted"),
        "the command reports a restart, got: {printed}"
    );
    assert!(
        printed.contains(
            "ended pid(s) 471 owned by approved; launchd's KeepAlive replaced it with pid(s) 1471"
        ),
        "the report names the process it ended and the one launchd started, got: {printed}"
    );
    // Why a kill counts as a restart is part of the report, not folklore.
    assert!(
        printed.contains(
            "that is what `launchctl kickstart -k` does to a KeepAlive job, minus the privilege it \
             needs: the process is replaced and the job is never unloaded"
        ),
        "the report explains why this is equivalent to a restart, got: {printed}"
    );

    // And the end state was checked on the host, not assumed.
    let out = host.stado(&[
        "service",
        "restart",
        OBJECT_API,
        "--host",
        "fake-mini",
        "--json",
    ]);
    assert!(
        out.status.success(),
        "second restart failed: {}",
        stderr(&out)
    );
    let report = &document(&out)[0];
    assert_eq!(
        report["postcondition"]["intent"],
        "the system daemon is running under a new pid"
    );
    assert_eq!(report["postcondition"]["state"], "met");
    assert_eq!(host.read_state("pids"), "2471 approved\n");
}

/// The same command against a daemon nothing would respawn: refused, with the
/// privileged command named, and the process still running.
#[test]
fn service_restart_refuses_a_daemon_nothing_would_respawn() {
    let host = Harness::new();

    // No KeepAlive at all: ending the process would leave the host down.
    host.state("keepalive", "MISSING\n");
    let out = host.stado(&["service", "restart", OBJECT_API, "--host", "fake-mini"]);
    assert!(!out.status.success(), "a refusal is not a success");
    assert!(
        stderr(&out).contains(
            "com.wisent.always-on.stado-object-api on fake-mini is a system LaunchDaemon at \
             /Library/LaunchDaemons/com.wisent.always-on.stado-object-api.plist; the approved \
             channel is unprivileged and cannot bootstrap it, and the unit declares no KeepAlive, \
             so ending its process would leave nothing to start another one and this host would go \
             from degraded to down. Restarting it needs one privileged command on the host: sudo \
             launchctl kickstart -k system/com.wisent.always-on.stado-object-api"
        ),
        "got: {}",
        stderr(&out)
    );
    assert_eq!(
        host.read_state("pids"),
        "471 approved\n",
        "a refused restart must not have touched the process"
    );

    // `KeepAlive false` is a declaration that launchd will NOT respawn it,
    // which reads as present to anything that only checks for the key.
    host.state("keepalive", "false\n");
    let out = host.stado(&["service", "restart", OBJECT_API, "--host", "fake-mini"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(
            "the unit declares KeepAlive false, so launchd will not start another process when \
             this one ends. Restarting it needs one privileged command on the host: sudo launchctl \
             kickstart -k system/com.wisent.always-on.stado-object-api"
        ),
        "got: {}",
        stderr(&out)
    );
    assert_eq!(host.read_state("pids"), "471 approved\n");

    // A KeepAlive dict may or may not respawn after a signal depending on its
    // keys, and this channel does not get to guess which.
    host.state("keepalive", "DICT\n");
    let out = host.stado(&["service", "restart", OBJECT_API, "--host", "fake-mini"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(
            "the unit declares a conditional KeepAlive, so whether launchd respawns it after a \
             signal depends on keys this channel must not guess at"
        ),
        "got: {}",
        stderr(&out)
    );
    assert_eq!(host.read_state("pids"), "471 approved\n");

    // KeepAlive is only half of it: an unprivileged login cannot signal a
    // process that belongs to another account.
    host.state("keepalive", "true\n");
    host.state("pids", "471 root\n");
    let out = host.stado(&["service", "restart", OBJECT_API, "--host", "fake-mini"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(
            "its process runs as another account (pid(s) 471), not as the approved user approved, \
             so this channel cannot signal it"
        ),
        "got: {}",
        stderr(&out)
    );
    assert_eq!(host.read_state("pids"), "471 root\n");
}

/// `service stop` of a system daemon used to report a stopped service that was
/// still serving: `sudo -n launchctl bootout` failed silently, the sweep ended
/// the process, launchd started another, and the end-state probe read the
/// system domain it cannot read as "no job". It is a refusal now.
#[test]
fn service_stop_refuses_a_system_daemon_it_cannot_boot_out() {
    let host = Harness::new();

    let out = host.stado(&["service", "stop", OBJECT_API, "--host", "fake-mini"]);
    assert!(!out.status.success(), "a refusal is not a success");
    assert!(
        stderr(&out).contains(
            "com.wisent.always-on.stado-object-api on fake-mini is a system LaunchDaemon at \
             /Library/LaunchDaemons/com.wisent.always-on.stado-object-api.plist; the approved \
             channel is unprivileged and cannot boot it out, and ending its process is not a stop \
             — launchd starts another one within seconds for a KeepAlive job. Stopping it needs \
             one privileged command on the host: sudo launchctl bootout \
             system/com.wisent.always-on.stado-object-api"
        ),
        "got: {}",
        stderr(&out)
    );
    assert_eq!(
        host.read_state("pids"),
        "471 approved\n",
        "a refused stop must leave the process alone"
    );
}

/// `host recover` used to print `status: ok` over a managed unit it had done
/// nothing about. The unit it skipped, and why, are now in the document, and
/// the status and exit code carry it.
#[test]
fn host_recover_reports_the_unit_it_could_not_bootstrap() {
    let host = Harness::new();

    let out = host.stado(&["host", "recover", "fake-mini"]);
    assert!(
        !out.status.success(),
        "a pass that left a managed unit unloaded is not a success: {}",
        stdout(&out)
    );
    let report = document(&out);
    assert_eq!(report["status"], "blocked");
    assert_eq!(report["agents"][BEACON], "needs_privileged_bootstrap");
    assert_eq!(report["blockers"], serde_json::json!([]));
    assert_eq!(
        report["skipped"],
        serde_json::json!([{
            "unit": BEACON,
            "reason": "declared at /Library/LaunchDaemons/com.wisent.host-health-beacon.plist in \
                       launchd's system domain; the approved channel logs in as an unprivileged \
                       user and cannot bootstrap it. Re-bootstrap it on the host with: sudo \
                       launchctl kickstart -k system/com.wisent.host-health-beacon"
        }]),
        "the skipped entry names the unit and the privileged command that loads it"
    );
    // The pass still ran: it is the accounting that changed, not the program.
    assert_eq!(report["host"], "fake-mini");
    assert!(report["disk_free_kb_after"].is_i64());
}

/// A declared unit file the host does not have is a blocker of its own, not a
/// line buried under `status: ok`. This is the finding that stood behind
/// control-host's stale beacons for twelve days.
#[test]
fn host_recover_carries_a_missing_unit_file_as_a_blocker() {
    let host = Harness::new();
    host.remove_daemon(BEACON);

    let out = host.stado(&["host", "recover", "fake-mini"]);
    assert!(!out.status.success());
    let report = document(&out);
    assert_eq!(report["status"], "blocked");
    assert_eq!(report["agents"][BEACON], "missing_plist");
    assert_eq!(report["skipped"], serde_json::json!([]));
    assert_eq!(
        report["blockers"],
        serde_json::json!([{
            "unit": BEACON,
            "finding": "missing_plist",
            "path": "/Library/LaunchDaemons/com.wisent.host-health-beacon.plist",
            "reason": "the declared unit file \
                       /Library/LaunchDaemons/com.wisent.host-health-beacon.plist is not on the \
                       host, so there is nothing to load and this host publishes no beacon. \
                       Reinstall it and load it with: sudo launchctl bootstrap system \
                       /Library/LaunchDaemons/com.wisent.host-health-beacon.plist"
        }]),
        "the blocker names the file, the consequence, and the command that fixes it"
    );
}

/// The unit path comes from the target's own declaration, and only falls back
/// to `MANAGED_AGENTS` for a host that declares nothing. A host whose beacon is
/// a per-login LaunchAgent is still reloaded, still reports `ok`, and still
/// exits 0 — which is what keeps `status: ok` worth reading.
#[test]
fn host_recover_still_reloads_an_undeclared_per_login_agent() {
    let host = Harness::new();
    host.host("fake-agent-mini");
    host.declare_agent(BEACON);

    let out = host.stado(&["host", "recover", "fake-agent-mini"]);
    assert!(
        out.status.success(),
        "a pass with nothing skipped and nothing blocking must exit 0: {}{}",
        stdout(&out),
        stderr(&out)
    );
    let report = document(&out);
    assert_eq!(report["status"], "ok");
    assert_eq!(report["agents"][BEACON], "restarted");
    assert_eq!(report["skipped"], serde_json::json!([]));
    assert_eq!(report["blockers"], serde_json::json!([]));

    // And the fallback really is the per-login path, not the daemon one the
    // other target declares: this host has no file under DAEMON_ROOT.
    assert!(
        !stdout(&out).contains(DAEMON_ROOT),
        "an undeclared host must not be reported against a system-domain path, got: {}",
        stdout(&out)
    );
}
