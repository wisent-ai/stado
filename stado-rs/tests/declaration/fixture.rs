//! The seeded registry, the beacon and the unit specification the declaration
//! cases share.
//!
//! Moved out of `main.rs` so each file stays inside the three hundred line
//! limit this repository enforces on itself — and so the area could be
//! repaired at all: a 566-line file cannot be edited under that gate.
//!
//! The repair: `weles` used to be declared with a null value whenever the host
//! was not graphical. The schema refuses a present-but-null `weles`, so one
//! case was red on `main` while claiming a corrected document validates. A key
//! that means nothing here is now omitted rather than declared empty.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The always-on row, as it appeared in the incident this area defends.
pub const ALWAYS_ON_HOST: &str = "control-host";
/// The interactive row, which keeps a graphical session.
pub const INTERACTIVE_HOST: &str = "laptop-host";

/// The mini's Stado agent, the unit the incident was about.
pub const AGENT: &str = "com.wisent.compute.service.stado-agent-mini";
/// The account that owns the declared agent, and the account the daemon
/// spelling has to keep running as.
pub const ACCOUNT: &str = "charles";
/// Where the agent is declared today: a per-account LaunchAgent.
pub const AGENT_PATH: &str =
    "/Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist";
/// Where the daemon spelling of the same unit belongs.
pub const AGENT_DAEMON_PATH: &str =
    "/Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist";
/// The interactive host's own user agent, declared in the domain it has.
pub const STREAM: &str = "com.wisent.transcript-lake-stream";
/// And where that agent is declared.
pub const STREAM_PATH: &str =
    "/Users/lukaszbartoszcze/Library/LaunchAgents/com.wisent.transcript-lake-stream.plist";
/// A unit on the always-on host that is already declared correctly, so a
/// silent row proves the check is about the domain and not about the host.
pub const WELES: &str = "com.wisent.always-on.weles";
/// And where it is declared.
pub const WELES_PATH: &str = "/Library/LaunchDaemons/com.wisent.always-on.weles.plist";
/// The program the live user-agent plist actually runs, read read-only
/// through `stado service show`: `ProgramArguments` is
/// `/Users/charles/.stado/bin/stado agent --auto`.
pub const AGENT_PROGRAM: &str = "/Users/charles/.stado/bin/stado";

/// The privileged command the finding names.
///
/// `install -m 644 -o root -g wheel` is the spelling `deploy/service.rs`'s
/// `ENSURE_BODY` already uses for a daemon, so the file an operator writes by
/// hand and the file the fleet writes have the same owner and mode.
/// `UserName` rides along because root reads a plist in
/// `/Library/LaunchDaemons`, and a daemon without that key would run the
/// account's own binary as uid 0 against an account-owned `~/.stado`.
pub const INSTALL_COMMAND: &str = "sudo /bin/sh -c '/usr/bin/install -m 644 -o root -g wheel \
     /Users/charles/Library/LaunchAgents/com.wisent.compute.service.stado-agent-mini.plist \
     /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist && \
     /usr/bin/plutil -insert UserName -string charles \
     /Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist'";

/// The one sentence both surfaces print, verbatim from a hand run.
pub fn sentence() -> String {
    format!(
        "com.wisent.compute.service.stado-agent-mini is declared in launchd's user domain \
         ({AGENT_PATH}), and {ALWAYS_ON_HOST} is declared always-on, so no account is logged in \
         graphically there, launchd builds no gui/<uid>, and system is the only domain that host \
         can load a unit into; install it there with one privileged command on the host: \
         {INSTALL_COMMAND}"
    )
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The `misdeclared-domain` rows in `registry doctor --json`.
pub fn domain_findings(out: &Output) -> Vec<serde_json::Value> {
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
pub fn service_row(out: &Output, unit: &str) -> serde_json::Value {
    let rows: serde_json::Value =
        serde_json::from_str(&stdout(out)).expect("service list --json prints one array");
    rows.as_array()
        .expect("an array of rows")
        .iter()
        .find(|row| row.get("unit_id").and_then(serde_json::Value::as_str) == Some(unit))
        .cloned()
        .unwrap_or_else(|| panic!("a row for {unit}"))
}
pub struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    pub fn new() -> Self {
        let harness = Self {
            dir: tempfile::tempdir().expect("temp root"),
        };
        for sub in ["storage", "storage/host_health", "home"] {
            std::fs::create_dir_all(harness.root().join(sub)).expect("temp subdirectory");
        }
        harness
    }

    pub fn root(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    /// Seed the canonical registry: one always-on host and one interactive
    /// host, each declaring one launchd unit at `path`.
    ///
    /// `graphical` declares Weles on the always-on host, which means that host
    /// intentionally keeps an Aqua login alive. When it does not, the key is
    /// left out entirely: the schema reads a declared `weles: null` as a
    /// malformed row, and rightly so.
    pub fn declare_registry(
        &self,
        graphical: bool,
        always_on_unit: &UnitSpec,
        interactive_unit: &UnitSpec,
    ) {
        let mut always_on = serde_json::json!({
            "name": ALWAYS_ON_HOST,
            "kind": "local",
            "ssh": "charles@10.9.9.21",
            "release_platform": "darwin-arm64",
            "hostnames": [format!("{ALWAYS_ON_HOST}.local")],
            "role": "always-on",
            "host_heuristic": "always-on",
            "services": [
                always_on_unit.to_json(),
                // Already a system LaunchDaemon: the shape every other
                // always-on unit on the mini has.
                UnitSpec::new(WELES, WELES_PATH).to_json(),
            ],
        });
        if graphical {
            always_on["weles"] = serde_json::json!({
                "enabled": true,
                "actions": ["generic_capture"],
            });
        }
        let document = serde_json::json!({
            "schema_version": 2,
            "targets": [
                always_on,
                {
                    "name": INTERACTIVE_HOST,
                    "kind": "local",
                    "ssh": "lukaszbartoszcze@10.9.9.22",
                    "release_platform": "darwin-arm64",
                    "hostnames": [format!("{INTERACTIVE_HOST}.local")],
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
    pub fn declare_beacon(&self, host: &str, active: &[&str]) {
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

    pub fn stado(&self, args: &[&str]) -> Output {
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
pub struct UnitSpec {
    label: String,
    path: String,
    program: String,
    args: Vec<String>,
}

impl UnitSpec {
    pub fn new(label: &str, path: &str) -> Self {
        Self {
            label: label.to_string(),
            path: path.to_string(),
            program: String::new(),
            args: Vec::new(),
        }
    }

    /// The corrected declaration also carries the program and arguments the
    /// unit runs, which is what makes it reinstallable from the document.
    pub fn running(mut self, program: &str, args: &[&str]) -> Self {
        self.program = program.to_string();
        self.args = args.iter().map(|arg| (*arg).to_string()).collect();
        self
    }

    pub fn to_json(&self) -> serde_json::Value {
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
