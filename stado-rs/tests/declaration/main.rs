//! A unit declaration against the launchd domain its host can actually have.
//!
//! `com.wisent.compute.service.stado-agent-mini` was declared as a user
//! LaunchAgent at `/Users/charles/Library/LaunchAgents/...` on
//! `control-host`. That host is declared always-on in both `role` and
//! `host_heuristic` and has no graphical session at all: `/dev/console` is
//! root's, `who` prints nothing, `loginwindow` runs as root, and the login's
//! own `launchctl list` holds no `com.wisent.*` label. So
//! `launchctl bootstrap user/501 <plist>` answers `Bootstrap failed: 5:
//! Input/output error` there, `gui/501` does not exist, and the declaration
//! named a domain that could never load the unit. Every other always-on unit
//! on that host is a system LaunchDaemon under `/Library/LaunchDaemons`.
//!
//! Nothing about that needs a host: the path says the domain and the target
//! says the host runs unattended, so it is a registry finding. These tests
//! defend that it is reported where an operator already looks — as a
//! `misdeclared-domain` row in `stado registry doctor` and as a
//! `declaration:` line under `stado service list` — that it stays silent for
//! an interactive host, where a user agent is exactly right, and that the
//! corrected declaration still validates.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir>. STADO_CONFIG
//! points at a nonexistent path so the developer's real config can never leak
//! into a test, and HOME is inside the temp dir. No host is contacted: both
//! commands under test answer from the registry document and the beacon
//! objects in that storage root alone.
//!
//! Every sentence asserted here was copied from a hand run against this
//! seeded state, never guessed.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The always-on host: declared unattended in both fields, the way
/// `control-host` is.
const ALWAYS_ON_HOST: &str = "mini-fake";
/// The interactive host: somebody is logged in, so a user LaunchAgent is the
/// right declaration and nothing here is a finding.
const INTERACTIVE_HOST: &str = "laptop-fake";
/// The mini's Stado agent, the unit the incident was about.
const AGENT: &str = "com.wisent.compute.service.stado-agent-mini";
/// The account that owns the declared agent, and the account the daemon
/// spelling has to keep running as.
const ACCOUNT: &str = "charles";
/// Where the agent is declared today: a per-account LaunchAgent.
const AGENT_PATH: &str =
    "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist";
/// Where the daemon spelling of the same unit belongs.
const AGENT_DAEMON_PATH: &str =
    "/Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist";
/// The interactive host's own user agent, declared in the domain it has.
const STREAM: &str = "com.wisent.transcript-lake-stream";
/// A unit on the always-on host that is already declared correctly, so a
/// silent row proves the check is about the domain and not about the host.
const WELES: &str = "com.wisent.always-on.weles";
/// The program the live user-agent plist actually runs, read read-only
/// through `stado service show`: `ProgramArguments` is
/// `/Users/charles/.stado/bin/stado agent --auto`.
const AGENT_PROGRAM: &str = "/Users/charles/.stado/bin/stado";

/// The privileged command the finding names.
///
/// `install -m 644 -o root -g wheel` is the spelling `deploy/service.rs`'s
/// `ENSURE_BODY` already uses for a daemon, so the file an operator writes by
/// hand and the file the fleet writes have the same owner and mode.
/// `UserName` rides along because root reads a plist in
/// `/Library/LaunchDaemons`, and a daemon without that key would run the
/// account's own binary as uid 0 against an account-owned `~/.stado`.
const INSTALL_COMMAND: &str = "sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel \
     /Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist \
     /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist && \
     /usr/bin/plutil -insert UserName -string charles \
     /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist'";

/// The one sentence both surfaces print, verbatim from a hand run.
fn sentence() -> String {
    format!(
        "com.wisent.compute.service.stado-agent-mini is declared in launchd's user domain \
         ({AGENT_PATH}), and {ALWAYS_ON_HOST} is declared always-on, so no account is logged in \
         graphically there, launchd builds no gui/<uid>, and system is the only domain that host \
         can load a unit into; install it there with one privileged command on the host: \
         {INSTALL_COMMAND}"
    )
}

/// A temp root carrying a storage backend with a registry and beacons in it.
struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let harness = Self {
            dir: tempfile::tempdir().expect("temp root"),
        };
        for sub in ["storage", "storage/host_health", "home"] {
            std::fs::create_dir_all(harness.root().join(sub)).expect("temp subdirectory");
        }
        harness
    }

    fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// Seed the canonical registry: one always-on host and one interactive
    /// host, each declaring one launchd unit at `path`.
    ///
    /// `hostnames` deliberately never names this machine, so no command under
    /// test can be routed at the developer's own launchd.
    fn declare_registry(&self, always_on_unit: &UnitSpec, interactive_unit: &UnitSpec) {
        let document = serde_json::json!({
            "schema_version": 2,
            "targets": [
                {
                    "name": ALWAYS_ON_HOST,
                    "kind": "local",
                    "ssh": "charles@10.9.9.21",
                    "release_platform": "darwin-arm64",
                    "hostnames": [format!("{ALWAYS_ON_HOST}.local")],
                    "slots": 1,
                    "role": "always-on",
                    "host_heuristic": "always-on",
                    "services": [
                        always_on_unit.to_json(),
                        // Already a system LaunchDaemon: the shape every other
                        // always-on unit on the mini has.
                        UnitSpec::new(WELES, "/Library/LaunchDaemons/com.wisent.always-on.weles.plist")
                            .to_json(),
                    ],
                },
                {
                    "name": INTERACTIVE_HOST,
                    "kind": "local",
                    "ssh": "lukaszbartoszcze@10.9.9.22",
                    "release_platform": "darwin-arm64",
                    "hostnames": [format!("{INTERACTIVE_HOST}.local")],
                    "slots": 1,
                    "role": "interactive",
                    "services": [interactive_unit.to_json()],
                },
            ],
            "coordinators": [],
        });
        std::fs::write(
            self.root().join("storage/registry.json"),
            serde_json::to_string_pretty(&document).expect("registry document"),
        )
        .expect("seed registry");
    }

    /// A fresh beacon for HOST reporting every unit in `active`, and nothing
    /// about any other declared unit.
    fn declare_beacon(&self, host: &str, active: &[&str]) {
        let units: serde_json::Map<String, serde_json::Value> = active
            .iter()
            .map(|unit| ((*unit).to_string(), serde_json::json!({"state": "active"})))
            .collect();
        let beacon = serde_json::json!({
            "host": host,
            "reported_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "units": units,
        });
        std::fs::write(
            self.root()
                .join("storage/host_health")
                .join(format!("{host}.json")),
            serde_json::to_string(&beacon).expect("beacon document"),
        )
        .expect("seed beacon");
    }

    fn stado(&self, args: &[&str]) -> Output {
        let root = self.root();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(args)
            .env_clear()
            .env("HOME", root.join("home"))
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", root.join("storage"))
            // A set-but-missing STADO_CONFIG disables config-file discovery.
            .env("STADO_CONFIG", root.join("storage/no-such-config.json"));
        cmd.output().expect("stado binary runs")
    }
}

/// One `services[]` element, as `stado service adopt` writes it.
struct UnitSpec {
    label: String,
    path: String,
    program: String,
    args: Vec<String>,
}

impl UnitSpec {
    fn new(label: &str, path: &str) -> Self {
        Self {
            label: label.to_string(),
            path: path.to_string(),
            program: String::new(),
            args: Vec::new(),
        }
    }

    /// The corrected declaration also carries the program and arguments the
    /// unit runs, which is what makes it reinstallable from the document.
    fn running(mut self, program: &str, args: &[&str]) -> Self {
        self.program = program.to_string();
        self.args = args.iter().map(|arg| (*arg).to_string()).collect();
        self
    }

    fn to_json(&self) -> serde_json::Value {
        let mut record = serde_json::json!({
            "name": self.label,
            "unit": "",
            "label": self.label,
            "path": self.path,
            "kind": "launchd",
            "managed_since": "2026-08-19T00:46:51.797832+00:00",
        });
        if !self.program.is_empty() {
            record["program"] = serde_json::json!(self.program);
            record["args"] = serde_json::json!(self.args);
        }
        record
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The `misdeclared-domain` rows in `registry doctor --json`.
fn domain_findings(out: &Output) -> Vec<serde_json::Value> {
    let report: serde_json::Value =
        serde_json::from_str(&stdout(out)).expect("registry doctor --json prints one object");
    report
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .expect("a findings array")
        .iter()
        .filter(|finding| {
            finding.get("finding").and_then(serde_json::Value::as_str) == Some("misdeclared-domain")
        })
        .cloned()
        .collect()
}

/// The row `service list --json` prints for one unit.
fn service_row(out: &Output, unit: &str) -> serde_json::Value {
    let rows: serde_json::Value =
        serde_json::from_str(&stdout(out)).expect("service list --json prints one array");
    rows.as_array()
        .expect("an array of rows")
        .iter()
        .find(|row| row.get("unit_id").and_then(serde_json::Value::as_str) == Some(unit))
        .cloned()
        .unwrap_or_else(|| panic!("a row for {unit}"))
}

/// A user agent declared on an always-on host is a `registry doctor` finding,
/// in one sentence naming the unit, both domains, and the privileged command.
#[test]
fn doctor_reports_a_user_agent_declared_on_an_always_on_host() {
    let harness = Harness::new();
    harness.declare_registry(
        &UnitSpec::new(AGENT, AGENT_PATH),
        &UnitSpec::new(
            STREAM,
            "/Users/lukaszbartoszcze/Library/LaunchAgents/com.wisent.transcript-lake-stream.plist",
        ),
    );
    harness.declare_beacon(ALWAYS_ON_HOST, &[WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    let out = harness.stado(&["registry", "doctor", "--json"]);
    let findings = domain_findings(&out);

    assert_eq!(findings.len(), 1, "one row, for the one misdeclared unit");
    assert_eq!(
        findings[0]
            .get("subject")
            .and_then(serde_json::Value::as_str),
        Some(ALWAYS_ON_HOST)
    );
    assert_eq!(
        findings[0]
            .get("detail")
            .and_then(serde_json::Value::as_str),
        Some(sentence().as_str())
    );
    // The sentence has to carry the command an operator runs next, verbatim,
    // and that command has to keep the job on the account that owns the agent:
    // a daemon without `UserName` runs the fleet's binary as uid 0 against an
    // account-owned `~/.stado`.
    let detail = findings[0]
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .expect("a detail sentence");
    assert!(
        detail.contains(INSTALL_COMMAND),
        "the finding names the privileged install command"
    );
    assert!(
        detail.contains(&format!(
            "/usr/bin/plutil -insert UserName -string {ACCOUNT} "
        )),
        "the install command keeps the daemon running as {ACCOUNT}"
    );
    // A divergence exits non-zero, the way every other doctor finding does.
    assert!(!out.status.success(), "doctor fails on a divergence");
}

/// The same declaration on an interactive host is correct, and the check says
/// nothing at all about it — not about the unit, and not about the host.
#[test]
fn doctor_stays_silent_for_a_user_agent_on_an_interactive_host() {
    let harness = Harness::new();
    // Both hosts declare a per-account LaunchAgent; only the always-on one is
    // a finding, so the interactive row is the control.
    harness.declare_registry(
        &UnitSpec::new(
            WELES,
            "/Library/LaunchDaemons/com.wisent.always-on.weles.plist",
        ),
        &UnitSpec::new(
            STREAM,
            "/Users/lukaszbartoszcze/Library/LaunchAgents/com.wisent.transcript-lake-stream.plist",
        ),
    );
    harness.declare_beacon(ALWAYS_ON_HOST, &[WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    let out = harness.stado(&["registry", "doctor", "--json"]);

    assert!(
        domain_findings(&out).is_empty(),
        "a user agent on an interactive host is the right declaration"
    );
    assert!(
        !stdout(&out).contains("misdeclared-domain"),
        "nothing in the report mentions the finding"
    );
}

/// `service list` prints the same sentence under the table, on the surface an
/// operator reads when the unit is missing from it.
#[test]
fn service_list_names_the_declared_domain_and_the_loadable_one() {
    let harness = Harness::new();
    harness.declare_registry(
        &UnitSpec::new(AGENT, AGENT_PATH),
        &UnitSpec::new(
            STREAM,
            "/Users/lukaszbartoszcze/Library/LaunchAgents/com.wisent.transcript-lake-stream.plist",
        ),
    );
    harness.declare_beacon(ALWAYS_ON_HOST, &[WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    let printed = stdout(&harness.stado(&["service", "list"]));

    assert!(
        printed.contains(&format!("declaration: {}", sentence())),
        "service list prints the declaration finding; got:\n{printed}"
    );
    // One line, for the one unit: the correctly declared daemon beside it and
    // the interactive host's agent produce nothing.
    assert_eq!(
        printed.matches("declaration: ").count(),
        1,
        "one declaration line; got:\n{printed}"
    );

    // The machine-readable half carries the same facts, for the dashboards
    // that read `--json` instead of the table.
    let out = harness.stado(&["service", "list", "--json"]);
    let row = service_row(&out, AGENT);
    let misdeclared = row
        .get("misdeclared_domain")
        .expect("the row carries the finding");
    assert_eq!(
        misdeclared
            .get("declared_domain")
            .and_then(serde_json::Value::as_str),
        Some("user")
    );
    assert_eq!(
        misdeclared
            .get("loadable_domain")
            .and_then(serde_json::Value::as_str),
        Some("system")
    );
    assert_eq!(
        misdeclared
            .get("daemon_path")
            .and_then(serde_json::Value::as_str),
        Some(AGENT_DAEMON_PATH)
    );
    assert_eq!(
        misdeclared
            .get("install_command")
            .and_then(serde_json::Value::as_str),
        Some(INSTALL_COMMAND)
    );
    assert_eq!(
        misdeclared
            .get("detail")
            .and_then(serde_json::Value::as_str),
        Some(sentence().as_str())
    );
    // The correctly declared daemon on the same host carries nothing.
    assert!(
        service_row(&out, WELES).get("misdeclared_domain").is_none(),
        "a system LaunchDaemon on an always-on host is not a finding"
    );
    assert!(
        service_row(&out, STREAM)
            .get("misdeclared_domain")
            .is_none(),
        "a user agent on an interactive host is not a finding"
    );
}

/// Correcting the declaration is what closes the finding, and the corrected
/// document — daemon path plus the program and arguments the unit runs — is
/// still a valid registry a `registry pull` round-trips.
#[test]
fn the_corrected_daemon_declaration_validates_and_closes_the_finding() {
    let harness = Harness::new();
    harness.declare_registry(
        &UnitSpec::new(AGENT, AGENT_DAEMON_PATH).running(AGENT_PROGRAM, &["agent", "--auto"]),
        &UnitSpec::new(
            STREAM,
            "/Users/lukaszbartoszcze/Library/LaunchAgents/com.wisent.transcript-lake-stream.plist",
        ),
    );
    harness.declare_beacon(ALWAYS_ON_HOST, &[AGENT, WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    // The document as `registry pull` hands it back, validated the way `push`
    // validates before it writes: a service entry carrying `program` and
    // `args` is registry-v2, not an unread key.
    let pulled = harness.stado(&["registry", "pull"]);
    assert!(pulled.status.success(), "registry pull answers");
    let pulled_path = harness.root().join("pulled.json");
    std::fs::write(&pulled_path, stdout(&pulled)).expect("write the pulled document");
    let validated = harness.stado(&[
        "registry",
        "validate",
        pulled_path.to_str().expect("a utf-8 path"),
    ]);
    assert!(
        validated.status.success(),
        "the corrected document validates; got:\n{}",
        String::from_utf8_lossy(&validated.stderr)
    );

    // And the finding is gone: the declaration and the host now agree.
    let doctor = harness.stado(&["registry", "doctor", "--json"]);
    assert!(
        domain_findings(&doctor).is_empty(),
        "a system LaunchDaemon on an always-on host is the right declaration"
    );
    let listed = stdout(&harness.stado(&["service", "list"]));
    assert!(
        !listed.contains("declaration: "),
        "service list prints no declaration finding; got:\n{listed}"
    );
}

/// The declaration finding is the cause of the `missing-plist` row for the
/// same unit, and only the cause survives: a beacon cannot report a unit
/// nothing ever loaded, and installing the plist where it is declared would
/// not change that.
#[test]
fn the_misdeclared_domain_replaces_the_missing_plist_symptom() {
    let harness = Harness::new();
    harness.declare_registry(
        &UnitSpec::new(AGENT, AGENT_PATH),
        &UnitSpec::new(
            STREAM,
            "/Users/lukaszbartoszcze/Library/LaunchAgents/com.wisent.transcript-lake-stream.plist",
        ),
    );
    // The beacon knows the daemon and says nothing about the agent, exactly
    // as `control-host`'s does.
    harness.declare_beacon(ALWAYS_ON_HOST, &[WELES]);
    harness.declare_beacon(INTERACTIVE_HOST, &[STREAM]);

    let out = harness.stado(&["registry", "doctor", "--json"]);
    let report: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("registry doctor --json prints one object");
    let findings = report
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .expect("a findings array");
    let about_agent: Vec<&serde_json::Value> = findings
        .iter()
        .filter(|finding| {
            finding
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|detail| detail.contains(AGENT))
        })
        .collect();

    assert_eq!(about_agent.len(), 1, "one cause, one row: {about_agent:?}");
    assert_eq!(
        about_agent[0]
            .get("finding")
            .and_then(serde_json::Value::as_str),
        Some("misdeclared-domain")
    );
    assert_eq!(
        about_agent[0]
            .get("detail")
            .and_then(serde_json::Value::as_str),
        Some(sentence().as_str())
    );
}
