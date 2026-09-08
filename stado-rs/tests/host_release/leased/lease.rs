//! One lease's whole life: taken, given a Stado to deliver over, declared,
//! read back, and destroyed.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

use crate::fixture::{stderr, stdout, BINARY};

use super::fleet::{document, fleet, leased, release_api, report};

/// A leased target, destroyed when the case leaves it — including through a
/// panic, so a failed assertion never leaves an account on a fleet host.
pub struct Lease {
    target: String,
    pub name: String,
    root: String,
    home: String,
    destroyed: bool,
}

impl Lease {
    pub fn take(target: &str, profile: &str) -> Self {
        let arguments = [
            "scratch", "create", "--host", target, "--profile", profile, "--ttl", "30m", "--json",
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
            home: field("home_path"),
            destroyed: false,
        }
    }

    fn stado(&self, arguments: &[&str]) -> Output {
        leased(&self.root, arguments)
    }

    /// Give the account Stado the way this repository documents giving it to
    /// a machine that has none: deliver `install-stado.sh` and the adapter
    /// that hands it its three environment coordinates, then run the adapter.
    ///
    /// This is a precondition, not the claim under test. `--apply` delivers
    /// `host-behind` and `unattested` rows only, and both need a version the
    /// reporter could read, so a blank account reads `unknown` and is
    /// delivered nothing. What the installer leaves behind is the state the
    /// capability calls `no-delivery-history`: installed, nothing staged.
    ///
    /// The receipt's exit status is deliberately not the evidence. `host
    /// run-attached` sends a script that assigns `status=`, which is
    /// read-only in zsh, so a program that succeeded on a zsh account is
    /// reported as having failed. The installer's own success line and
    /// [`Lease::installed`] are the evidence instead.
    pub fn bootstrap(&self, version: &str, platform: &str) {
        let run = Command::new("uuidgen").output().expect("uuidgen runs");
        let run = String::from_utf8_lossy(&run.stdout).trim().to_lowercase();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        for (source, name) in [
            (manifest.join("../install-stado.sh"), "install-stado.sh"),
            (
                manifest.join("tests/host_release/bootstrap_leased_account.sh"),
                "bootstrap_leased_account.sh",
            ),
        ] {
            let source = source.to_string_lossy().to_string();
            let destination = format!(".stado/work/runs/{run}/{name}");
            let arguments = [
                "host",
                "deliver",
                &self.name,
                &source,
                &destination,
                "--json",
            ];
            let delivered = document(&self.stado(&arguments), &arguments);
            assert_eq!(delivered["status"], "delivered", "{delivered}");
        }
        let program = format!(
            "{}/.stado/work/runs/{run}/bootstrap_leased_account.sh",
            self.home
        );
        let api = release_api();
        let arguments = [
            "host",
            "run-attached",
            &self.name,
            "--program",
            &program,
            "--arg",
            &api,
            "--arg",
            version,
            "--arg",
            platform,
            "--json",
        ];
        let output = self.stado(&arguments);
        let receipt = report(&output, "the bootstrap");
        assert_eq!(
            receipt["stdout"].as_str().unwrap_or_default().trim(),
            format!(
                "installed Stado {version} for {platform} in {}/.stado/bin",
                self.home
            ),
            "the documented installer did not report installing {version}: {receipt}{}",
            stderr(&output)
        );
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
    pub fn host_state(&self, extra: &[&str]) -> (Value, Value) {
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
        (state, row)
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
