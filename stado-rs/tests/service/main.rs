//! `stado service declare` contract tests against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir> and a
//! STADO_CONFIG pointing at a nonexistent path, so the developer's real
//! config can never leak into a test. HOME is a tempdir too, so the
//! registry last-known-good cache can never reach the operator's real one.
//!
//! What is defended here: a declaration the contract accepts lands in the
//! directory with its source, run spec, endpoints and consumers intact, and
//! every refusal — bad name, unknown host, non-immutable digest, missing
//! consumers, unknown verify kind — refuses with its exact sentence and
//! leaves the document untouched.
//!
//! The Linux lifecycle cases keep that same real-binary boundary. Their only
//! fake is the SSH destination: the generated remote program executes against
//! a systemd host model that records every init-system call, so the tests prove
//! a unit under `/etc/systemd/system` is adopted, restarted, and read through
//! the system manager rather than silently redirected to `systemd --user`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A valid registry-v2 document: one host, plus a service directory whose
/// authority that host publishes. `services` starts empty; the first declare
/// fills it.
const SEEDED_REGISTRY: &str = r#"{
    "schema_version": 2,
    "targets": [
        {
            "name": "w1",
            "kind": "local",
            "ssh": "u@10.0.0.1",
            "release_platform": "linux-amd64",
            "hostnames": ["w1.local"],
            "slots": 1
        }
    ],
    "coordinators": [],
    "service_directory": {
        "authority": {"target": "w1", "command": "/usr/local/bin/stado"},
        "generation": 1,
        "services": {}
    }
}"#;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A HOME and a storage root, both temporary, plus the seeded document.
struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    fn new() -> Self {
        let fleet = Self {
            home: tempfile::tempdir().unwrap(),
            storage: tempfile::tempdir().unwrap(),
        };
        std::fs::write(fleet.registry_blob(), SEEDED_REGISTRY).unwrap();
        fleet
    }

    fn registry_blob(&self) -> PathBuf {
        self.storage.path().join("registry.json")
    }

    fn document(&self) -> serde_json::Value {
        let body = std::fs::read_to_string(self.registry_blob()).expect("registry blob exists");
        serde_json::from_str(&body).expect("registry stays JSON")
    }

    /// Write one declaration file and return its path.
    fn declaration_file(&self, body: &str) -> PathBuf {
        let path = self.storage.path().join("declaration.json");
        std::fs::write(&path, body).unwrap();
        path
    }

    fn stado(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_stado"));
        cmd.args(args)
            .env("HOME", self.home.path())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env(
                "STADO_CONFIG",
                self.storage.path().join("no-such-config.json"),
            )
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR");
        cmd.output().expect("stado binary runs")
    }

    fn declare(&self, body: &str) -> Output {
        let path = self.declaration_file(body);
        self.stado(&[
            "service",
            "declare",
            "--file",
            path.to_str().expect("declaration path is UTF-8"),
        ])
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn valid_declaration() -> String {
    format!(
        r#"{{
    "name": "example-serving",
    "host": "w1",
    "port": 8080,
    "source": {{"artifact": "stado://releases/example-serving/1.0.0/linux-amd64", "sha256": "{DIGEST}"}},
    "run": {{"args": ["serve"]}},
    "consumers": {{"example-backend": {{"capabilities": ["model-routing"]}}}}
}}"#
    )
}

#[test]
fn declare_writes_the_contract_into_the_directory() {
    let fleet = Fleet::new();

    let out = fleet.declare(&valid_declaration());
    assert!(out.status.success(), "declare failed: {}", stderr(&out));

    let document = fleet.document();
    let entry = &document["service_directory"]["services"]["example-serving"];
    assert_eq!(entry["active_host"], "w1");
    assert_eq!(entry["endpoints"]["w1"]["url"], "http://127.0.0.1:8080");
    assert_eq!(
        entry["declaration"]["source"]["artifact"],
        "stado://releases/example-serving/1.0.0/linux-amd64"
    );
    assert_eq!(entry["declaration"]["source"]["sha256"], DIGEST);
    assert_eq!(entry["declaration"]["run"]["args"][0], "serve");
    assert_eq!(
        entry["consumers"]["example-backend"]["capabilities"][0],
        "model-routing"
    );
}

#[test]
fn declare_refuses_a_digest_that_is_not_immutable() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace(DIGEST, "ABC123");

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted a short digest");
    assert!(
        stderr(&out).contains("must be 64 lowercase hex characters"),
        "got: {}",
        stderr(&out)
    );
    // A refused declaration never moves the document.
    assert_eq!(read_document_raw(&fleet), SEEDED_REGISTRY);
}

#[test]
fn declare_refuses_a_host_outside_the_registry() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace("\"host\": \"w1\"", "\"host\": \"w9\"");

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted an unknown host");
    assert!(
        stderr(&out).contains("which is not a registry target"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn declare_refuses_a_service_without_consumers() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace(
        "    \"run\": {\"args\": [\"serve\"]},\n    \"consumers\": {\"example-backend\": {\"capabilities\": [\"model-routing\"]}}\n",
        "    \"run\": {\"args\": [\"serve\"]}\n",
    );

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted no consumers");
    assert!(
        stderr(&out).contains("'consumers' is required"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn declare_refuses_an_unknown_verify_kind() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace(
        r#"    "consumers"#,
        r#"    "verify": {"kind": "dns"},
    "consumers"#,
    );

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted verify kind dns");
    assert!(
        stderr(&out).contains("unknown value 'dns'"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn declare_refuses_a_name_with_empty_edges() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace("example-serving", "-example-");

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted a bad name");
    assert!(
        stderr(&out).contains("must be a lowercase identifier without empty edges"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn declare_refuses_a_declaration_without_an_endpoint() {
    let fleet = Fleet::new();
    let body = valid_declaration().replace("    \"port\": 8080,\n", "");

    let out = fleet.declare(&body);
    assert!(!out.status.success(), "declare accepted no endpoint");
    assert!(
        stderr(&out).contains("pass 'endpoints' or the 'port' shorthand"),
        "got: {}",
        stderr(&out)
    );
}

fn read_document_raw(fleet: &Fleet) -> String {
    std::fs::read_to_string(fleet.registry_blob()).expect("registry blob exists")
}

/// A registry target, spelled the way the canonical document spells one: only
/// `name` and `kind` are required, and every field this plan reads carries a
/// serde default.
fn target(name: &str, role: &str, platform: &str) -> stado::targets::ComputeTarget {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "kind": "local",
        "role": role,
        "release_platform": platform,
    }))
    .expect("a registry target parses")
}

fn agent_plan(target: &stado::targets::ComputeTarget) -> stado::deploy::service::DeployPlan {
    stado::deploy::service::plan_deploy_labelled(
        target,
        "stado-agent-mini",
        "com.wisent.compute.service.stado-agent-mini",
        "/home/operator/.stado/bin/stado",
        &["agent".to_string(), "--auto".to_string()],
        &[],
    )
    .expect("plan renders")
}

const AGENT_DAEMON_FILE: &str =
    "/Library/LaunchDaemons/com.wisent.compute.service.stado-agent-mini.plist";

#[test]
fn as_daemon_ensure_addresses_the_system_daemon_file() {
    let plan = agent_plan(&target("lukasz-macbook", "interactive", "darwin-arm64"));

    // Default placement follows the declaration and the per-login fallback:
    // no path is forced, so the remote prelude keeps its search order.
    assert!(
        stado::deploy::service::ensure_unit_path(&plan).is_empty(),
        "default ensure must not force a unit file"
    );

    // --as-daemon pins the one domain that survives on an always-on host
    // with no graphical session, whatever the declaration says.
    let mut daemon = plan.clone();
    daemon.force_daemon = true;
    assert_eq!(
        stado::deploy::service::ensure_unit_path(&daemon),
        AGENT_DAEMON_FILE
    );
}

const SYSTEMD_AGENT: &str = "wisent-compute-agent.service";

const SYSTEMD_REGISTRY: &str = r#"{
  "schema_version": 2,
  "targets": [{
    "name": "linux-builder",
    "kind": "local",
    "ssh": "approved@10.9.9.20",
    "release_platform": "linux-amd64",
    "hostnames": ["linux-builder.invalid"],
    "slots": 2
  }],
  "coordinators": []
}"#;

/// The product's fixed remote program arrives on stdin. Only host-owned
/// executables are redirected; the program itself is the production one.
///
/// The one filesystem substitution changes the existence probe, not the path
/// the program resolves: the report must still carry
/// `/etc/systemd/system/<unit>`, while the fixture stays inside the tempdir.
const SYSTEMD_FAKE_SSH: &str = r#"#!/bin/sh
PATH="$STADO_FAKE_HOST_BIN:/usr/bin:/bin"; export PATH
/usr/bin/sed \
  -e 's#\[ -f "/etc/systemd/system/\$unit" \]#[ -f "$STADO_FAKE_STATE/system/$unit" ]#' \
  -e 's#/usr/bin/uname#uname#g' \
  -e 's#/usr/bin/id#id#g' \
  -e 's#/usr/bin/stat#stat#g' \
  -e 's#/usr/bin/systemctl#systemctl#g' \
  -e 's#/usr/bin/journalctl#journalctl#g' \
  -e 's#/usr/bin/loginctl#loginctl#g' \
  -e 's#/usr/bin/sudo#sudo#g' \
  -e 's#/bin/sudo#sudo#g' \
  -e 's#/bin/sleep#sleep#g' \
  | /bin/bash -s
"#;

const SYSTEMD_FAKE_UNAME: &str = "#!/bin/sh\necho Linux\n";

const SYSTEMD_FAKE_ID: &str = r#"#!/bin/sh
case "${1:-}" in
  -u)
    if [ "${2:-}" = root ]; then echo 0; else echo 1000; fi
    ;;
  -un) echo approved ;;
  *) echo 1000 ;;
esac
"#;

const SYSTEMD_FAKE_STAT: &str = r#"#!/bin/sh
if [ "${1:-}" = -c ] && [ "${2:-}" = %U ]; then
  echo root
  exit 0
fi
exit 1
"#;

/// A real call log plus the minimum systemd state the restart contract reads.
const SYSTEMD_FAKE_SYSTEMCTL: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$STADO_FAKE_STATE/systemctl.log"
case "${1:-}" in
  cat|daemon-reload) exit 0 ;;
  restart)
    printf 'active\n' > "$STADO_FAKE_STATE/active"
    exit 0
    ;;
  is-active)
    [ -f "$STADO_FAKE_STATE/active" ]
    ;;
  show)
    [ -f "$STADO_FAKE_STATE/active" ] || exit 1
    printf '570420\n'
    ;;
  *) exit 64 ;;
esac
"#;

const SYSTEMD_FAKE_JOURNALCTL: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$STADO_FAKE_STATE/journalctl.log"
printf '%s\n' 'agent stopped after release handoff'
"#;

/// The approved host account has passwordless, non-interactive systemd
/// administration. The `-n` is kept in its own log because dropping it would
/// let a lifecycle command wait forever for a password on an SSH pipe.
const SYSTEMD_FAKE_SUDO: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$STADO_FAKE_STATE/sudo.log"
[ "${1:-}" = -n ] || exit 65
shift
exec "$@"
"#;

const SYSTEMD_FAKE_LOGINCTL: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$STADO_FAKE_STATE/loginctl.log"
exit 66
"#;

const SYSTEMD_FAKE_SLEEP: &str = "#!/bin/sh\nexit 0\n";

fn systemd_tool(directory: &Path, name: &str, body: &str) {
    let path = directory.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct SystemdHost {
    root: tempfile::TempDir,
}

impl SystemdHost {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        for child in ["ssh-bin", "host-bin", "state/system", "storage", "home"] {
            std::fs::create_dir_all(root.path().join(child)).unwrap();
        }
        systemd_tool(&root.path().join("ssh-bin"), "ssh", SYSTEMD_FAKE_SSH);
        let host_bin = root.path().join("host-bin");
        systemd_tool(&host_bin, "uname", SYSTEMD_FAKE_UNAME);
        systemd_tool(&host_bin, "id", SYSTEMD_FAKE_ID);
        systemd_tool(&host_bin, "stat", SYSTEMD_FAKE_STAT);
        systemd_tool(&host_bin, "systemctl", SYSTEMD_FAKE_SYSTEMCTL);
        systemd_tool(&host_bin, "journalctl", SYSTEMD_FAKE_JOURNALCTL);
        systemd_tool(&host_bin, "sudo", SYSTEMD_FAKE_SUDO);
        systemd_tool(&host_bin, "loginctl", SYSTEMD_FAKE_LOGINCTL);
        systemd_tool(&host_bin, "sleep", SYSTEMD_FAKE_SLEEP);

        std::fs::write(
            root.path().join("state/system").join(SYSTEMD_AGENT),
            "[Service]\nExecStart=/home/approved/.stado/bin/stado agent --auto\n",
        )
        .unwrap();
        std::fs::write(root.path().join("storage/registry.json"), SYSTEMD_REGISTRY).unwrap();
        let key = root.path().join("state/ssh-key");
        std::fs::write(&key, "-----BEGIN OPENSSH PRIVATE KEY-----\nfake\n").unwrap();
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        Self { root }
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env_clear()
            .env("HOME", self.root.path().join("home"))
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                    self.root.path().join("ssh-bin").display()
                ),
            )
            .env("STADO_FAKE_HOST_BIN", self.root.path().join("host-bin"))
            .env("STADO_FAKE_STATE", self.root.path().join("state"))
            .env(
                "STADO_HOST_SSH_KEY_FILE",
                self.root.path().join("state/ssh-key"),
            )
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.path().join("storage"))
            .env(
                "STADO_CONFIG",
                self.root.path().join("storage/no-such-config.json"),
            )
            .output()
            .expect("stado binary runs")
    }

    fn document(&self) -> serde_json::Value {
        let body = std::fs::read_to_string(self.root.path().join("storage/registry.json")).unwrap();
        serde_json::from_str(&body).unwrap()
    }

    fn declare_systemd_agent(&self) {
        let mut document = self.document();
        document["targets"][0]["services"] = serde_json::json!([{
            "name": SYSTEMD_AGENT,
            "unit": SYSTEMD_AGENT,
            "label": "",
            "path": format!("/etc/systemd/system/{SYSTEMD_AGENT}"),
            "kind": "systemd",
            "source": "registry",
            "managed_since": "2026-09-04T00:00:00Z"
        }]);
        std::fs::write(
            self.root.path().join("storage/registry.json"),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
    }

    fn state(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.path().join("state").join(name)).unwrap()
    }
}

/// The live Ubuntu builder carried this exact split: its agent was active as a
/// machine unit while adoption searched only the approved login's user
/// manager. The command now records the machine path, and restart addresses
/// that same manager without `--user` or a meaningless linger mutation.
#[test]
fn systemd_system_unit_is_adopted_and_restarted_in_the_system_scope() {
    let host = SystemdHost::new();
    let adopted = host.stado(&[
        "service",
        "adopt",
        SYSTEMD_AGENT,
        "--host",
        "linux-builder",
        "--json",
    ]);
    assert!(
        adopted.status.success(),
        "adopt failed: {}",
        stderr(&adopted)
    );

    let document = host.document();
    let service = &document["targets"][0]["services"][0];
    assert_eq!(service["unit"], SYSTEMD_AGENT);
    assert_eq!(service["kind"], "systemd");
    assert_eq!(
        service["path"],
        format!("/etc/systemd/system/{SYSTEMD_AGENT}")
    );

    let restarted = host.stado(&[
        "service",
        "restart",
        SYSTEMD_AGENT,
        "--host",
        "linux-builder",
        "--json",
    ]);
    assert!(
        restarted.status.success(),
        "restart failed: {}{}",
        String::from_utf8_lossy(&restarted.stdout),
        stderr(&restarted)
    );
    let reports: serde_json::Value = serde_json::from_slice(&restarted.stdout).unwrap();
    let report = &reports[0];
    assert_eq!(report["status"], "restarted");
    assert_eq!(report["detail"], "systemd system scope");
    assert_eq!(report["launchd_domain"]["name"], "system");
    assert_eq!(report["postcondition"]["state"], "met");

    let calls = host.state("systemctl.log");
    assert!(
        calls
            .lines()
            .any(|line| line == format!("restart {SYSTEMD_AGENT}")),
        "system manager was not asked to restart the unit:\n{calls}"
    );
    assert!(
        calls.lines().all(|line| !line.contains("--user")),
        "a system unit was sent to the per-user manager:\n{calls}"
    );
    assert!(
        !host.root.path().join("state/loginctl.log").exists(),
        "a system unit must not alter per-user linger state"
    );
    assert!(
        host.state("sudo.log")
            .lines()
            .all(|line| line.starts_with("-n systemctl ")),
        "system management must stay non-interactive"
    );
}

/// A machine unit writes to the system journal. `service logs` must address
/// that journal without `--user`, or it reports `-- No entries --` while the
/// failure that stopped the fleet's only Linux builder remains unread.
#[test]
fn systemd_system_unit_logs_come_from_the_system_journal() {
    let host = SystemdHost::new();
    host.declare_systemd_agent();

    let logged = host.stado(&[
        "service",
        "logs",
        SYSTEMD_AGENT,
        "--host",
        "linux-builder",
        "--lines",
        "20",
        "--json",
    ]);
    assert!(
        logged.status.success(),
        "logs failed: {}{}",
        String::from_utf8_lossy(&logged.stdout),
        stderr(&logged)
    );
    let reports: serde_json::Value = serde_json::from_slice(&logged.stdout).unwrap();
    let report = &reports[0];
    assert_eq!(report["origin"], format!("journalctl -u {SYSTEMD_AGENT}"));
    assert_eq!(
        report["lines"],
        serde_json::json!(["agent stopped after release handoff"])
    );

    let calls = host.state("journalctl.log");
    assert_eq!(calls.trim(), format!("-u {SYSTEMD_AGENT} -n 20 --no-pager"));
    let sudo = host.state("sudo.log");
    assert!(
        sudo.lines()
            .any(|line| line == format!("-n journalctl -u {SYSTEMD_AGENT} -n 20 --no-pager")),
        "system journal access must stay non-interactive:\n{sudo}"
    );
}
