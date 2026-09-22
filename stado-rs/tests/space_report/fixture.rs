//! The isolated registry, the home it points at, and the command run against
//! them, so every case here drives the real binary and keeps its bytes.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

/// The isolated registry names one target, and the assertions read it back.
pub(crate) const TARGET: &str = "space-report-fixture";

/// Directories a Mac always has, so the fixture's `PATH` finds `df`, `tr` and
/// the shell the remote program runs under.
pub(crate) const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Schema versions the product's own writers use: `config_file` for the
/// configuration document and `targets::REGISTRY_SCHEMA_VERSION` for the
/// registry. Named here rather than spelled inside the fixture documents, so
/// a reader can see which contract each number belongs to.
const CONFIG_SCHEMA_VERSION: i64 = 1;
const REGISTRY_SCHEMA_VERSION: i64 = 2;

pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) storage: PathBuf,
    pub(crate) config: PathBuf,
}

fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write fixture file");
    let mut permissions = fs::metadata(path).expect("stat fixture file").permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions).expect("restrict fixture file");
}

fn hostname() -> String {
    let output = Command::new("/bin/hostname")
        .output()
        .expect("read this machine's hostname");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-output/space-report");
        fs::create_dir_all(&evidence).unwrap();
        let root = tempfile::Builder::new()
            .prefix("run-")
            .tempdir_in(evidence)
            .unwrap()
            .keep();
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        fs::write(root.join("revision.txt"), revision.stdout).unwrap();
        let home = root.join("home");
        let storage = root.join("storage");
        for directory in [&home, &storage, &root.join("tmp")] {
            fs::create_dir_all(directory).expect("create fixture directory");
        }
        let config = root.join("config.json");
        write_private(
            &config,
            &serde_json::to_vec_pretty(&json!({
                "schema_version": CONFIG_SCHEMA_VERSION,
                "storage": {"backend": "local", "local": {"path": storage}},
            }))
            .expect("render fixture config"),
        );
        write_private(
            &storage.join("registry.json"),
            &serde_json::to_vec_pretty(&json!({
                "schema_version": REGISTRY_SCHEMA_VERSION,
                "targets": [{
                    "name": TARGET,
                    "kind": "local",
                    "ssh": null,
                    "release_platform": "darwin-arm64",
                    "hostnames": [hostname()],
                    "services": [],
                }],
                "coordinators": [],
            }))
            .expect("render fixture registry"),
        );
        Self {
            root,
            home,
            storage,
            config,
        }
    }

    /// The command with the fixture's own home, storage and configuration,
    /// and the attribution walk held to `budget_seconds`, where `0` means the
    /// walk is not attempted at all. A case that needs another bound adds it
    /// to this command rather than building a second one.
    pub(crate) fn command(&self, budget_seconds: &str, extra: &[&str]) -> Command {
        let mut args = vec!["space", "report", TARGET];
        args.extend_from_slice(extra);
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(&args)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", SYSTEM_PATH)
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", &self.config)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("NO_COLOR", "1")
            .env("STADO_INVENTORY_BUDGET_SECONDS", budget_seconds);
        command
    }

    /// The report with the attribution walk held to `budget_seconds`, where
    /// `0` means the walk is not attempted at all.
    pub(crate) fn report(&self, budget_seconds: &str, extra: &[&str]) -> Output {
        let output = self
            .command(budget_seconds, extra)
            .output()
            .expect("run stado space report");
        self.retain(extra, &output);
        output
    }

    /// Every run keeps its own bytes beside the fixture, named by the shape
    /// of the answer it asked for.
    pub(crate) fn retain(&self, extra: &[&str], output: &Output) {
        let name = if extra.contains(&"--json") {
            "json"
        } else {
            "text"
        };
        fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        fs::write(
            self.root.join(format!("{name}.exit")),
            format!("{:?}", output.status.code()),
        )
        .unwrap();
    }

    pub(crate) fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.home);
        let _ = fs::remove_dir_all(&self.storage);
    }
}
