//! A disposable target's actual release declaration, first delivery and
//! destruction. No bootstrap helper or operator binary is a prerequisite.

use std::path::Path;
use std::process::Output;

use serde_json::Value;

use crate::fixture::{stderr, stdout, BINARY};

use super::fleet::{document, fleet, leased, report};

/// A leased target, destroyed when the case leaves it — including through a
/// panic, so a failed assertion never leaves an account on a fleet host.
pub struct Lease {
    target: String,
    pub name: String,
    root: String,
    destroyed: bool,
    _run_root: tempfile::TempDir,
}

impl Lease {
    pub fn take(target: &str, profile: &str) -> Self {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/host-release-test-runs");
        std::fs::create_dir_all(&directory).expect("create repository test directory");
        let run_root = tempfile::tempdir_in(directory).expect("an isolated lease directory");
        let registry_root = run_root.path().join("registry");
        let arguments = [
            "scratch",
            "create",
            "--host",
            target,
            "--profile",
            profile,
            "--root",
            registry_root
                .to_str()
                .expect("the repository path is UTF-8"),
            "--ttl",
            "30m",
            "--json",
        ];
        let report = document(&fleet(&arguments), &arguments);
        assert_eq!(report["status"], "leased", "{report}");
        assert_eq!(report["account"], "created", "{report}");
        let field = |key: &str| {
            report[key]
                .as_str()
                .unwrap_or_else(|| panic!("the lease report carries no {key}: {report}"))
                .to_string()
        };
        Self {
            target: target.to_string(),
            name: field("name"),
            root: field("storage_root"),
            destroyed: false,
            _run_root: run_root,
        }
    }

    fn stado(&self, arguments: &[&str]) -> Output {
        leased(&self.root, arguments)
    }

    /// What the host itself says about the managed binary, through a command
    /// that reads the machine and never sees a delivery report.
    pub fn installed(&self) -> Value {
        let arguments = ["host", "inventory", &self.name, "--json"];
        document(&self.stado(&arguments), &arguments)["managed_binaries"]
            .as_array()
            .unwrap_or_else(|| panic!("the inventory reports no managed binaries"))
            .iter()
            .find(|row| row["name"] == BINARY)
            .cloned()
            .unwrap_or_else(|| panic!("the inventory does not name {BINARY}"))
    }

    /// Declare a version, and read it back out of the lease's own emitted
    /// registry: a declaration that did not land there means nothing.
    pub fn declare(&self, version: &str) {
        let arguments = [
            "release",
            "declare-version",
            "--host",
            &self.name,
            "--binary",
            BINARY,
            "--version",
            version,
            "--json",
        ];
        document(&self.stado(&arguments), &arguments);
        let registry: Value = serde_json::from_str(
            &std::fs::read_to_string(Path::new(&self.root).join("registry.json"))
                .expect("the emitted registry is readable"),
        )
        .expect("the emitted registry stays JSON");
        assert_eq!(
            registry["targets"][0]["managed_versions"][BINARY], version,
            "the declaration has to land in the lease's own registry: {registry}"
        );
    }

    /// One `host-state` report and the single declared binary's row in it.
    pub fn host_state(&self, extra: &[&str]) -> (Output, Value, Value) {
        let mut arguments = vec!["release", "host-state", "--host", &self.name, "--json"];
        arguments.extend_from_slice(extra);
        let output = self.stado(&arguments);
        let state = report(&output, "host-state");
        let binaries = state["binaries"]
            .as_array()
            .expect("the report carries the binaries it examined");
        assert_eq!(
            binaries.len(),
            usize::from(true),
            "one declared binary was expected: {state}"
        );
        let row = binaries[0].clone();
        (output, state, row)
    }

    /// The one delivery the pass recorded for the managed binary.
    pub fn released(applied: &Value) -> Value {
        applied["releases"]
            .as_array()
            .expect("the pass records what it attempted")
            .iter()
            .find(|entry| entry["binary"] == BINARY)
            .cloned()
            .unwrap_or_else(|| panic!("no delivery of {BINARY} is recorded: {applied}"))
    }

    /// Destroy the lease, and prove the host is clear of it: the account, the
    /// home and the record are read back by a probe after the delete, which
    /// is why all three are asserted rather than the exit status alone.
    pub fn destroy(&mut self) {
        let arguments = [
            "scratch",
            "destroy",
            &self.name,
            "--host",
            &self.target,
            "--json",
        ];
        let report = document(&fleet(&arguments), &arguments);
        self.destroyed = true;
        assert_eq!(report["status"], "destroyed", "{report}");
        assert_eq!(report["account"], "absent", "{report}");
        assert_eq!(report["home"], "absent", "{report}");
        assert_eq!(report["record"], "absent", "{report}");
        let listed = ["scratch", "list", "--host", &self.target, "--json"];
        let leases = document(&fleet(&listed), &listed);
        assert!(
            !leases["leases"]
                .as_array()
                .unwrap_or(&Vec::new())
                .iter()
                .any(|row| row["name"] == self.name.as_str()),
            "the host still reports the destroyed lease: {leases}"
        );
        assert!(
            !Path::new(&self.root).exists(),
            "the emitted registry root went with the lease"
        );
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.destroyed {
            return;
        }
        let arguments = [
            "scratch",
            "destroy",
            &self.name,
            "--host",
            &self.target,
            "--json",
        ];
        let swept = fleet(&arguments);
        if !swept.status.success() {
            eprintln!(
                "the lease {} outlived its case and could not be destroyed: {}{}",
                self.name,
                stdout(&swept),
                stderr(&swept)
            );
        }
    }
}
