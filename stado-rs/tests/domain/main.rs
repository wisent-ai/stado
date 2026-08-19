//! Which launchd domain a unit belongs to, and what a command may claim about
//! a unit it did not load.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! into a test, and HOME is inside the temp dir so nothing the remote program
//! expands can reach the operator's own `~/.stado` or `~/Library/LaunchAgents`.
//!
//! The host is a fake, and it is a fake in one place: `ssh`. The product pipes
//! its fixed remote program to `ssh` on stdin, so a script named `ssh` on PATH
//! receives that program byte for byte and runs it against the fake `launchctl`
//! / `stat` / `PlistBuddy` / `pgrep` / `ps` / `kill` in `host-bin`. Those tools
//! are NOT on the caller's PATH — only the far side of that hop sees them —
//! because `deploy/host_channel.rs` runs a target whose `hostnames` match this
//! machine locally instead of over ssh, and a fake `hostname` visible to the
//! product would turn every one of these tests into a run against the
//! developer's own launchd.
//!
//! What the fake host models is the two things macOS actually exposes about a
//! graphical session, and the two states charless-mac-mini has been in:
//!
//! - `state/console` is the owner of `/dev/console`, which is the login user
//!   while a graphical session exists and `root` at the login window.
//! - `state/gui` is whether launchd has a `gui/501` domain at all.
//! - `state/jobs` is launchd's job table, one `<domain>/<label> <pid>` per
//!   line, so `launchctl print` answers about a domain the way the host does:
//!   a job in `gui/501` is invisible from `user/501` and the other way round.
//! - `state/procs` is the process table, one `<pid> <user> <argv...>` per line.
//!   Every unit in these tests runs ONE program with different arguments,
//!   which is the shape of every Stado service on the mini and the reason a
//!   restart scoped by program path ended eight of them.
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
  -e "s#/usr/bin/stat#stat#g" \
  -e "s#/bin/kill#$STADO_FAKE_HOST_BIN/kill#g" \
  | /bin/bash -s
"#;

const FAKE_UNAME: &str = "#!/bin/sh\necho Darwin\n";

const FAKE_HOSTNAME: &str = "#!/bin/sh\ncat \"$STADO_FAKE_STATE/hostname\"\n";

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

/// The one read that says whether anybody is logged in graphically:
/// `stat -f%Su /dev/console` is the console's owner.
const FAKE_STAT: &str = r#"#!/bin/sh
case "${1:-}" in
  -f%Su) cat "$STADO_FAKE_STATE/console" ;;
  *) exit 1 ;;
esac
"#;

/// launchd, in one script: a job table per domain, `print` that answers about
/// one domain only, `bootstrap` that can refuse the way the background domain
/// refuses an agent needing the login session (`Load failed: 5: Input/output
/// error`), and `kickstart -k` that replaces a pid without unloading.
///
/// `state/refuse_bootstrap` is that refusal; `state/silent_bootstrap` is the
/// nastier one — exit 0 with no job created, which is what a command that
/// trusted the exit status reported as a restart.
const FAKE_LAUNCHCTL: &str = r##"#!/bin/sh
S="$STADO_FAKE_STATE"
jobs="$S/jobs"
procs="$S/procs"
is_domain() {
  case "$1" in
    gui/*/*|user/*/*) return 1 ;;
    system|gui/*|user/*) return 0 ;;
  esac
  return 1
}
next_pid() {
  last=$(/usr/bin/awk 'END {print $1 + 0}' "$procs" 2>/dev/null)
  echo $((last + 100))
}
argv_of() {
  /usr/bin/tr '\n' ' ' < "$S/argv/$1" | /usr/bin/sed 's/ $//'
}
case "${1:-}" in
  print)
    target="${2:-}"
    if is_domain "$target"; then
      case "$target" in
        system) echo "Could not print domain: 1: Operation not permitted" >&2; exit 1 ;;
        gui/*) [ -f "$S/gui" ] || { echo "Could not find domain for $target" >&2; exit 113; } ;;
      esac
      exit 0
    fi
    pid=$(/usr/bin/awk -v t="$target" '$1 == t {print $2}' "$jobs" 2>/dev/null)
    if [ -z "$pid" ]; then
      echo "Could not find service \"${target##*/}\" in domain for ${target%/*}" >&2
      exit 113
    fi
    echo "$target = {"
    echo "	pid = $pid"
    echo "}"
    exit 0
    ;;
  bootstrap)
    domain="${2:-}"; plist="${3:-}"
    base="${plist##*/}"; label="${base%.plist}"
    [ -f "$S/silent_bootstrap" ] && exit 0
    if [ -f "$S/refuse_bootstrap" ]; then
      case "$domain" in
        user/*) echo "Load failed: 5: Input/output error"; exit 5 ;;
      esac
    fi
    if /usr/bin/awk -v t="$domain/$label" '$1 == t {found = 1} END {exit !found}' "$jobs" 2>/dev/null; then
      echo "Bootstrap failed: 37: Operation already in progress"; exit 37
    fi
    pid=$(next_pid)
    echo "$domain/$label $pid" >> "$jobs"
    printf '%s approved %s\n' "$pid" "$(argv_of "$label")" >> "$procs"
    exit 0
    ;;
  bootout)
    target="${2:-}"
    pid=$(/usr/bin/awk -v t="$target" '$1 == t {print $2}' "$jobs" 2>/dev/null)
    [ -n "$pid" ] || { echo "Boot-out failed: 113: Could not find specified service" >&2; exit 113; }
    /usr/bin/awk -v t="$target" '$1 != t' "$jobs" > "$jobs.next" && mv "$jobs.next" "$jobs"
    /usr/bin/awk -v p="$pid" '$1 != p' "$procs" > "$procs.next" && mv "$procs.next" "$procs"
    exit 0
    ;;
  kickstart)
    target="${3:-}"
    old=$(/usr/bin/awk -v t="$target" '$1 == t {print $2}' "$jobs" 2>/dev/null)
    [ -n "$old" ] || { echo "Could not find service \"${target##*/}\"" >&2; exit 113; }
    label="${target##*/}"
    new=$(next_pid)
    /usr/bin/awk -v t="$target" -v p="$new" '$1 == t {$2 = p} {print}' "$jobs" > "$jobs.next" && mv "$jobs.next" "$jobs"
    /usr/bin/awk -v p="$old" '$1 != p' "$procs" > "$procs.next" && mv "$procs.next" "$procs"
    printf '%s approved %s\n' "$new" "$(argv_of "$label")" >> "$procs"
    exit 0
    ;;
esac
exit 0
"##;

/// `pgrep -f "^<prefix>"`: the pattern these programs use is always a path
/// prefix, so a prefix match is the whole of it.
const FAKE_PGREP: &str = r#"#!/bin/sh
pattern="${2:-}"
pattern="${pattern#^}"
while read -r pid user rest; do
  [ -n "$pid" ] || continue
  case "$rest" in
    "$pattern"*) echo "$pid" ;;
  esac
done < "$STADO_FAKE_STATE/procs"
"#;

/// `ps -p <pid> -o command=` is the read that makes a pid attributable to one
/// unit rather than to one binary, so the fake answers it faithfully.
const FAKE_PS: &str = r#"#!/bin/sh
want=""; field=""; prev=""
for arg in "$@"; do
  case "$prev" in -p) want="$arg" ;; esac
  case "$arg" in command=|comm=|user=) field="$arg" ;; esac
  case "$arg" in -p) prev=-p ;; *) prev="$arg" ;; esac
done
while read -r pid user rest; do
  [ "$pid" = "$want" ] || continue
  case "$field" in
    user=) echo "$user" ;;
    comm=) echo "${rest%% *}" ;;
    *) echo "$rest" ;;
  esac
done < "$STADO_FAKE_STATE/procs"
"#;

/// Waiting is the one thing a test must not do for real.
const FAKE_SLEEP: &str = "#!/bin/sh\nexit 0\n";

/// launchd's `KeepAlive`, in nine lines: a signalled process leaves the table,
/// and with `state/respawn` a replacement running the same argv takes its
/// place.
const FAKE_KILL: &str = r#"#!/bin/sh
pid="$2"
procs="$STADO_FAKE_STATE/procs"
line=$(/usr/bin/awk -v p="$pid" '$1 == p {print}' "$procs")
[ -n "$line" ] || exit 1
/usr/bin/awk -v p="$pid" '$1 != p' "$procs" > "$procs.next"
if [ -f "$STADO_FAKE_STATE/respawn" ]; then
  user=$(printf '%s' "$line" | /usr/bin/awk '{print $2}')
  argv=$(printf '%s' "$line" | /usr/bin/cut -d' ' -f3-)
  printf '%s %s %s\n' "$((pid + 1000))" "$user" "$argv" >> "$procs.next"
fi
mv "$procs.next" "$procs"
"#;

const FAKE_PLUTIL: &str = r#"#!/bin/sh
if [ "${1:-}" = "-lint" ]; then exit 0; fi
case "${2:-}" in
  KeepAlive)
    raw=$(cat "$STADO_FAKE_STATE/keepalive" 2>/dev/null || echo MISSING)
    case "$raw" in
      MISSING) exit 1 ;;
      *) if [ "${3:-}" = raw ]; then printf '%s\n' "$raw"; else printf '<%s/>\n' "$raw"; fi ;;
    esac ;;
  EnvironmentVariables.*)
    file="$STADO_FAKE_STATE/env/${2#EnvironmentVariables.}"
    [ -f "$file" ] || exit 1
    cat "$file" ;;
  *) exit 1 ;;
esac
"#;

/// The unit's declared argv, out of `state/argv/<label>`, in PlistBuddy's own
/// array framing — the spelling the product parses.
const FAKE_PLISTBUDDY: &str = r#"#!/bin/sh
plist=""
for arg in "$@"; do plist="$arg"; done
base="${plist##*/}"
file="$STADO_FAKE_STATE/argv/${base%.plist}"
case "${2:-}" in
  "Print :ProgramArguments")
    [ -f "$file" ] || { echo "Print: Entry, \":ProgramArguments\", Does Not Exist"; exit 1; }
    echo "Array {"
    /usr/bin/sed 's/^/    /' "$file"
    echo "}"
    ;;
  "Print :ProgramArguments:0")
    [ -f "$file" ] || exit 1
    /usr/bin/sed -n '1p' "$file"
    ;;
  *) echo "Print: Entry, \"${2:-}\", Does Not Exist"; exit 1 ;;
esac
"#;

/// The one binary every unit on this host runs.
const PROGRAM: &str = "/Users/approved/.stado/bin/stado";
/// This login's own LaunchAgent — the shape of the mini's Stado agent.
const AGENT: &str = "com.wisent.compute.service.stado-agent-mini";
/// The object API, which this fleet keeps as a system LaunchDaemon.
const OBJECT_API: &str = "com.wisent.always-on.stado-object-api";
/// The unit every recovery pass reloads.
const BEACON: &str = "com.wisent.host-health-beacon";

fn tool(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl Harness {
    /// A temp root with the fake host seeded for the state charless-mac-mini is
    /// in: nobody logged in graphically, so `/dev/console` is root's and
    /// launchd has no `gui/501`.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for child in [
            "ssh-bin",
            "host-bin",
            "state",
            "state/argv",
            "state/env",
            "state/LaunchDaemons",
            "storage",
            "home/Library/LaunchAgents",
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

        // `deploy/ssh_key.rs` materializes a target-scoped key for every
        // host-channel command. STADO_HOST_SSH_KEY_FILE is the product's own
        // owner-only override for a broker that cannot answer, which is
        // exactly this test's situation.
        let key = root.join("state/ssh-key");
        std::fs::write(&key, "-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();

        let harness = Self { dir };
        harness.state("hostname", "fake-agent\n");
        harness.state("console", "root\n");
        harness.state("keepalive", "true\n");
        harness.state("jobs", "");
        harness.state("procs", "");
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

    /// Somebody is logged in graphically: the console is this login's, and
    /// launchd has the `gui/501` domain that only exists with that session.
    fn graphical_session(&self) {
        self.state("console", "approved\n");
        self.state("gui", "");
    }

    /// The refusal the background per-user domain answers a LaunchAgent with.
    fn background_domain_refuses_agents(&self) {
        self.state("refuse_bootstrap", "");
    }

    /// `launchctl bootstrap` exiting 0 and creating nothing.
    fn bootstrap_says_nothing(&self) {
        self.state("silent_bootstrap", "");
    }

    /// A unit file in this login's LaunchAgents, with the argv it declares.
    fn declare_agent(&self, label: &str, args: &[&str]) -> String {
        let path = self
            .root()
            .join("home/Library/LaunchAgents")
            .join(format!("{label}.plist"));
        std::fs::write(
            &path,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\
                 <key>Label</key><string>{label}</string>\n</dict></plist>\n"
            ),
        )
        .unwrap();
        self.declare_argv(label, args);
        path.to_string_lossy().into_owned()
    }

    /// A unit file in `/Library/LaunchDaemons`, where the fake host keeps the
    /// system domain's plists.
    fn declare_daemon(&self, label: &str, args: &[&str]) -> String {
        std::fs::write(
            self.root()
                .join("state/LaunchDaemons")
                .join(format!("{label}.plist")),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\
                 <key>Label</key><string>{label}</string>\n\
                 <key>UserName</key><string>approved</string>\n\
                 <key>KeepAlive</key><true/>\n</dict></plist>\n"
            ),
        )
        .unwrap();
        self.declare_argv(label, args);
        format!("/Library/LaunchDaemons/{label}.plist")
    }

    fn declare_argv(&self, label: &str, args: &[&str]) {
        let mut argv = vec![PROGRAM.to_string()];
        argv.extend(args.iter().map(|arg| (*arg).to_string()));
        self.state(&format!("argv/{label}"), &format!("{}\n", argv.join("\n")));
    }

    /// The registry this fake host is declared in. `hostnames` deliberately
    /// does not name this machine, so the channel really does go through the
    /// fake `ssh`.
    fn declare_registry(&self, services: &[(&str, &str)]) {
        let entries: Vec<String> = services
            .iter()
            .map(|(label, path)| {
                format!(
                    "{{\"name\": \"{label}\", \"unit\": \"\", \"label\": \"{label}\", \
                     \"path\": \"{path}\", \"kind\": \"launchd\", \
                     \"managed_since\": \"2026-08-01T00:00:00Z\"}}"
                )
            })
            .collect();
        std::fs::write(
            self.root().join("storage/registry.json"),
            format!(
                "{{\n  \"schema_version\": 2,\n  \"targets\": [\n    {{\n      \
                 \"name\": \"fake-agent\",\n      \"kind\": \"local\",\n      \
                 \"ssh\": \"approved@10.9.9.11\",\n      \
                 \"release_platform\": \"darwin-arm64\",\n      \
                 \"hostnames\": [\"fake-agent.local\"],\n      \"slots\": 1,\n      \
                 \"services\": [{}]\n    }}\n  ],\n  \"coordinators\": []\n}}",
                entries.join(", ")
            ),
        )
        .unwrap();
    }

    /// The scoped beacon configuration the recovery pass validates before it
    /// loads that unit.
    fn declare_beacon_config(&self) {
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
            ("STADO_BIN", PROGRAM),
        ] {
            self.state(&format!("env/{name}"), value);
        }
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

/// The one report `service restart --json` prints, for the one host.
fn report(out: &Output) -> serde_json::Value {
    let printed = stdout(out);
    let documents: serde_json::Value =
        serde_json::from_str(&printed).expect("service restart --json prints one JSON array");
    documents
        .get(0)
        .cloned()
        .expect("one report, for the one host")
}

/// A LaunchAgent of a user who is logged in graphically lives in `gui/<uid>`:
/// the console is that user's and launchd has the domain. The restart acts
/// there, and the end state is read there.
#[test]
fn a_user_agent_of_a_graphical_session_is_resolved_to_gui() {
    let host = Harness::new();
    host.graphical_session();
    let path = host.declare_agent(AGENT, &["agent", "--host", "fake-agent"]);
    host.declare_registry(&[(AGENT, &path)]);

    let out = host.stado(&["service", "restart", AGENT, "--host", "fake-agent", "--json"]);
    assert!(
        out.status.success(),
        "restart failed: {}{}",
        stdout(&out),
        stderr(&out)
    );
    let report = report(&out);
    assert_eq!(report["launchd_domain"]["name"], "gui/501");
    assert_eq!(report["launchd_domain"]["status"], "graphical");
    assert_eq!(
        report["launchd_domain"]["reason"],
        "approved owns /dev/console and launchd has gui/501, so a LaunchAgent of this login loads \
         there"
    );
    assert_eq!(report["status"], "restarted");

    // The state that matters: launchd has the job in gui/501, and the end
    // state was read in that domain rather than in the one an ssh login sits
    // in.
    assert_eq!(
        host.read_state("jobs"),
        format!("gui/501/{AGENT} 100\n"),
        "the job must be bootstrapped in gui/501"
    );
    assert_eq!(report["postcondition"]["state"], "met");
    assert_eq!(
        report["postcondition"]["detail"],
        format!("gui/501/{AGENT} pid 100")
    );
}

/// With nobody logged in graphically there is no `gui/501` to load into, and
/// the report says which domain was left and why. The fallback is not a
/// refusal in itself: an agent the background domain accepts is restarted
/// there, and the end state is read there.
#[test]
fn a_user_agent_without_a_graphical_session_falls_back_to_the_background_domain() {
    let host = Harness::new();
    let path = host.declare_agent(AGENT, &["agent", "--host", "fake-agent"]);
    host.declare_registry(&[(AGENT, &path)]);

    let out = host.stado(&["service", "restart", AGENT, "--host", "fake-agent", "--json"]);
    assert!(
        out.status.success(),
        "an agent the background domain accepts is restarted there: {}{}",
        stdout(&out),
        stderr(&out)
    );
    let report = report(&out);
    assert_eq!(report["launchd_domain"]["name"], "user/501");
    assert_eq!(report["launchd_domain"]["status"], "fallback");
    assert_eq!(
        report["launchd_domain"]["reason"],
        "/dev/console belongs to root, not approved: no graphical session, so gui/501 does not \
         exist and a LaunchAgent has only the background domain user/501"
    );
    assert_eq!(report["status"], "restarted");
    assert_eq!(
        host.read_state("jobs"),
        format!("user/501/{AGENT} 100\n")
    );
    assert_eq!(
        report["postcondition"]["detail"],
        format!("user/501/{AGENT} pid 100")
    );
}

/// The refusal. `launchctl bootstrap user/501` answers `Load failed: 5:
/// Input/output error` for an agent that needs the login session, and the old
/// program answered that by exec'ing the unit's argv in the background and
/// reporting `restarted: direct process <pid>` beside `postcondition unmet`.
/// Now the status is the failure, the sentence names the domain and the reason,
/// and nothing runs that no unit owns.
#[test]
fn a_restart_that_leaves_no_job_in_the_domain_it_used_refuses() {
    let host = Harness::new();
    host.background_domain_refuses_agents();
    let path = host.declare_agent(AGENT, &["agent", "--host", "fake-agent"]);
    host.declare_registry(&[(AGENT, &path)]);

    let out = host.stado(&["service", "restart", AGENT, "--host", "fake-agent", "--json"]);
    assert!(
        !out.status.success(),
        "a unit launchd does not have is not a restarted service: {}",
        stdout(&out)
    );
    let report = report(&out);
    assert_eq!(report["status"], "not_loaded");
    assert_eq!(
        report["detail"],
        format!(
            "{AGENT} is not loaded in user/501: Load failed: 5: Input/output error. Nothing was \
             started outside launchd, because a process no unit owns dies with the login that \
             spawned it and is not a restarted service. /dev/console belongs to root, not \
             approved: no graphical session, so gui/501 does not exist and a LaunchAgent has only \
             the background domain user/501"
        )
    );
    assert_eq!(report["postcondition"]["state"], "unmet");
    assert_eq!(
        report["postcondition"]["detail"],
        format!("no job at user/501/{AGENT}"),
        "the end state is read in the domain the restart used"
    );

    // The state that matters: no job, and — the whole point — no process
    // either. `direct process <pid>` is not a fallback any more.
    assert_eq!(host.read_state("jobs"), "");
    assert_eq!(
        host.read_state("procs"),
        "",
        "a refused restart must not leave a bare process behind"
    );
    assert!(
        !stdout(&out).contains("direct process"),
        "got: {}",
        stdout(&out)
    );
}

/// The mini's own state on 2026-08-19: a process serving under no unit at all,
/// which the sweep ends on the way to reloading the unit — and then the unit
/// cannot be loaded. The refusal has to say that the process was ended, or an
/// operator reads it as "nothing happened" while the host runs nothing.
#[test]
fn a_refusal_names_the_disowned_process_the_restart_ended() {
    let host = Harness::new();
    host.background_domain_refuses_agents();
    let path = host.declare_agent(AGENT, &["agent", "--host", "fake-agent"]);
    host.declare_registry(&[(AGENT, &path)]);
    host.state(
        "procs",
        &format!("5426 approved {PROGRAM} agent --host fake-agent\n"),
    );

    let out = host.stado(&["service", "restart", AGENT, "--host", "fake-agent", "--json"]);
    assert!(!out.status.success(), "got: {}", stdout(&out));
    let report = report(&out);
    assert_eq!(report["status"], "not_loaded");
    assert_eq!(
        report["detail"],
        format!(
            "{AGENT} is not loaded in user/501: ended disowned process(es) 5426; Load failed: 5: \
             Input/output error. Nothing was started outside launchd, because a process no unit \
             owns dies with the login that spawned it and is not a restarted service. /dev/console \
             belongs to root, not approved: no graphical session, so gui/501 does not exist and a \
             LaunchAgent has only the background domain user/501"
        )
    );
    assert_eq!(host.read_state("procs"), "");
    assert_eq!(host.read_state("jobs"), "");
}

/// `launchctl bootstrap` returning 0 and leaving no job is the same failure
/// wearing a success's exit status, and it is what a pass that trusted that
/// status called `restarted`.
#[test]
fn a_bootstrap_that_reports_success_and_creates_no_job_is_not_a_restart() {
    let host = Harness::new();
    host.graphical_session();
    host.bootstrap_says_nothing();
    let path = host.declare_agent(AGENT, &["agent", "--host", "fake-agent"]);
    host.declare_registry(&[(AGENT, &path)]);

    let out = host.stado(&["service", "restart", AGENT, "--host", "fake-agent", "--json"]);
    assert!(!out.status.success(), "got: {}", stdout(&out));
    let report = report(&out);
    assert_eq!(report["status"], "not_loaded");
    assert_eq!(
        report["detail"],
        format!(
            "{AGENT} is not loaded in gui/501: launchctl bootstrap said nothing and left no job. \
             Nothing was started outside launchd, because a process no unit owns dies with the \
             login that spawned it and is not a restarted service"
        ),
        "a graphical session has no fallback reason to add"
    );
    assert_eq!(host.read_state("jobs"), "");
}

/// A unit in `/Library/LaunchDaemons` belongs to launchd's `system` domain,
/// whatever the login session looks like, and the report says so.
#[test]
fn a_system_daemon_is_resolved_to_the_system_domain() {
    let host = Harness::new();
    host.state("respawn", "");
    let path = host.declare_daemon(OBJECT_API, &["dashboard", "--bind", "127.0.0.1", "--port", "8765"]);
    host.declare_registry(&[(OBJECT_API, &path)]);
    host.state("procs", &format!("62398 approved {PROGRAM} dashboard --bind 127.0.0.1 --port 8765\n"));

    let out = host.stado(&[
        "service", "restart", OBJECT_API, "--host", "fake-agent", "--json",
    ]);
    assert!(
        out.status.success(),
        "restart failed: {}{}",
        stdout(&out),
        stderr(&out)
    );
    let report = report(&out);
    assert_eq!(report["launchd_domain"]["name"], "system");
    assert_eq!(report["launchd_domain"]["status"], "system");
    assert_eq!(report["status"], "restarted");
    assert_eq!(report["postcondition"]["state"], "met");
}

/// The blast radius. Every Stado service on charless-mac-mini runs
/// `/Users/charles/.stado/bin/stado`, and a restart that resolved its pids by
/// that path ended eight processes on 2026-08-19 — the object API, the host's
/// resolver holding 17600/17601/17612/17621, and a bare agent — then reported
/// one unit restarted with a met end state. The argv is what tells them apart.
#[test]
fn a_host_running_two_units_off_one_program_restarts_one_of_them() {
    let host = Harness::new();
    host.state("respawn", "");
    let daemon = host.declare_daemon(
        OBJECT_API,
        &["dashboard", "--bind", "127.0.0.1", "--port", "8765"],
    );
    let agent = host.declare_agent(AGENT, &["agent", "--host", "fake-agent"]);
    host.declare_registry(&[(OBJECT_API, &daemon), (AGENT, &agent)]);
    host.state(
        "procs",
        &format!(
            "5426 approved {PROGRAM} agent --host fake-agent\n\
             5490 approved {PROGRAM} resolver serve --target fake-agent\n\
             62398 approved {PROGRAM} dashboard --bind 127.0.0.1 --port 8765\n"
        ),
    );

    let out = host.stado(&[
        "service", "restart", OBJECT_API, "--host", "fake-agent", "--json",
    ]);
    assert!(
        out.status.success(),
        "restart failed: {}{}",
        stdout(&out),
        stderr(&out)
    );
    let report = report(&out);
    assert_eq!(
        report["detail"],
        "ended pid(s) 62398 owned by approved; launchd's KeepAlive replaced it with pid(s) 63398 \
         after 1s — that is what `launchctl kickstart -k` does to a KeepAlive job, minus the \
         privilege it needs: the process is replaced and the job is never unloaded",
        "exactly the one pid running this unit's argv"
    );

    // The state that matters: the two siblings running the same program are
    // untouched, and only the restarted unit's process was replaced.
    assert_eq!(
        host.read_state("procs"),
        format!(
            "5426 approved {PROGRAM} agent --host fake-agent\n\
             5490 approved {PROGRAM} resolver serve --target fake-agent\n\
             63398 approved {PROGRAM} dashboard --bind 127.0.0.1 --port 8765\n"
        )
    );
}

/// The same scoping in the sweep a restart runs before it reloads a unit: a
/// process of the unit being restarted that no job owns is ended, and a
/// sibling unit's live process running the same program is not.
#[test]
fn the_disowned_sweep_ends_only_the_processes_of_the_unit_it_restarts() {
    let host = Harness::new();
    host.graphical_session();
    let object = host.declare_agent(
        OBJECT_API,
        &["dashboard", "--bind", "127.0.0.1", "--port", "8765"],
    );
    let agent = host.declare_agent(AGENT, &["agent", "--host", "fake-agent"]);
    host.declare_registry(&[(OBJECT_API, &object), (AGENT, &agent)]);
    // The agent is loaded and running; the object API has a process no job
    // owns, which is what the sweep exists for.
    host.state("jobs", &format!("gui/501/{AGENT} 501\n"));
    host.state(
        "procs",
        &format!(
            "501 approved {PROGRAM} agent --host fake-agent\n\
             502 approved {PROGRAM} dashboard --bind 127.0.0.1 --port 8765\n"
        ),
    );

    let out = host.stado(&[
        "service", "restart", OBJECT_API, "--host", "fake-agent", "--json",
    ]);
    assert!(
        out.status.success(),
        "restart failed: {}{}",
        stdout(&out),
        stderr(&out)
    );
    assert_eq!(report(&out)["status"], "restarted");
    assert_eq!(
        host.read_state("jobs"),
        format!("gui/501/{AGENT} 501\ngui/501/{OBJECT_API} 601\n"),
        "the sibling's job is untouched and the restarted unit is loaded"
    );
    assert_eq!(
        host.read_state("procs"),
        format!(
            "501 approved {PROGRAM} agent --host fake-agent\n\
             601 approved {PROGRAM} dashboard --bind 127.0.0.1 --port 8765\n"
        ),
        "pid 501 belongs to another unit and must survive a restart of this one"
    );
}

/// `host recover` used to print `status: ok` with `launchd_domain: {name:
/// user/501, status: fallback}` underneath it over a unit it had not loaded.
/// The fallback is the reason that unit cannot be loaded, so it is a blocker
/// with that reason, and the pass is `blocked`.
#[test]
fn host_recover_carries_the_domain_fallback_as_a_blocker() {
    let host = Harness::new();
    host.background_domain_refuses_agents();
    host.declare_beacon_config();
    let beacon = host.declare_agent(BEACON, &["host-health-beacon"]);
    host.declare_registry(&[(BEACON, &beacon)]);

    let out = host.stado(&["host", "recover", "fake-agent"]);
    assert!(
        !out.status.success(),
        "a pass that loaded no managed unit is not a success: {}",
        stdout(&out)
    );
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("host recover prints one JSON document");
    assert_eq!(document["status"], "blocked");
    assert_eq!(
        document["launchd_domain"],
        serde_json::json!({
            "name": "user/501",
            "status": "fallback",
            "reason": "/dev/console belongs to root, not approved: no graphical session, so \
                       gui/501 does not exist and a LaunchAgent has only the background domain \
                       user/501",
        })
    );
    assert_eq!(
        document["blockers"],
        serde_json::json!([{
            "unit": BEACON,
            "finding": "bootstrap_failed:5:Load failed: 5: Input/output error",
            "path": beacon,
            "domain": "user/501",
            "reason": format!(
                "{BEACON} could not be loaded in user/501, the only domain this login has: \
                 /dev/console belongs to root, not approved: no graphical session, so gui/501 does \
                 not exist and a LaunchAgent has only the background domain user/501. Until \
                 somebody is logged in graphically on this host, or the unit is declared in \
                 launchd's system domain, nothing will run it"
            ),
        }]),
        "the blocker names the domain, the reason, and what would change it"
    );
    assert_eq!(document["skipped"], serde_json::json!([]));
}

/// And the other half of that, or `blocked` would mean nothing: with a
/// graphical session the same pass loads the same unit in `gui/501`, reports
/// `ok`, and exits 0.
#[test]
fn host_recover_reports_ok_when_the_agent_domain_is_the_graphical_one() {
    let host = Harness::new();
    host.graphical_session();
    host.declare_beacon_config();
    let beacon = host.declare_agent(BEACON, &["host-health-beacon"]);
    host.declare_registry(&[(BEACON, &beacon)]);

    let out = host.stado(&["host", "recover", "fake-agent"]);
    assert!(
        out.status.success(),
        "a pass with nothing skipped and nothing blocking must exit 0: {}{}",
        stdout(&out),
        stderr(&out)
    );
    let document: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("host recover prints one JSON document");
    assert_eq!(document["status"], "ok");
    assert_eq!(document["agents"][BEACON], "restarted");
    assert_eq!(document["launchd_domain"]["name"], "gui/501");
    assert_eq!(document["launchd_domain"]["status"], "graphical");
    assert_eq!(document["blockers"], serde_json::json!([]));
    // `bootstrap` gives the job pid 100, and the `kickstart -k` the pass runs
    // after it replaces that process — which is what the table ends with.
    assert_eq!(
        host.read_state("jobs"),
        format!("gui/501/{BEACON} 200\n"),
        "the unit is loaded in the domain the report names"
    );
}
