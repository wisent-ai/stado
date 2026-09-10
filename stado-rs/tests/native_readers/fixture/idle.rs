//! The idle unit: a LaunchAgent launchd holds and never starts, declaring the
//! delivered binary. It reproduces charless-mac-mini's
//! `com.wisent.compute.service.com.wisent.always-on.stado-resolver` on
//! 2026-09-10, which `launchctl print system` listed with pid 0 while the
//! 0.20.1 delivery read that zero as a live pid and failed on it.

use super::*;

impl Fixture {
    pub(crate) fn bootstrap_idle(&self) {
        self.write_idle_plist();
        let output = Command::new("/bin/launchctl")
            .args(["bootstrap", &self.domain])
            .arg(&self.idle_plist)
            .output()
            .expect("launchctl bootstrap runs for the idle unit");
        assert!(
            output.status.success(),
            "launchctl bootstrap of the idle unit failed: {}",
            said(&output)
        );
        assert!(
            self.idle_is_loaded(),
            "launchd does not hold the idle unit after bootstrap"
        );
        assert_eq!(
            self.launchd_pid(&self.idle_label),
            None,
            "the idle unit started although it declares neither RunAtLoad nor KeepAlive"
        );
    }

    pub(crate) fn idle_is_loaded(&self) -> bool {
        Command::new("/bin/launchctl")
            .args(["print", &format!("{}/{}", self.domain, self.idle_label)])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// The product's own inventory row for `label`, as `service list
    /// --undeclared --json` reports it for the fixture host.
    pub(crate) fn inventory_row(&self, label: &str) -> serde_json::Value {
        let output = self
            .command(&["service", "list", "--undeclared", "--json"])
            .output()
            .expect("undeclared inventory command runs");
        assert!(
            output.status.success(),
            "the undeclared inventory failed: {}",
            said(&output)
        );
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
                panic!(
                    "the undeclared inventory is not JSON ({error}): {}",
                    said(&output)
                )
            });
        report["undeclared"]
            .as_array()
            .unwrap_or_else(|| panic!("the inventory carries no undeclared rows: {report}"))
            .iter()
            .find(|row| row["label"] == label)
            .cloned()
            .unwrap_or_else(|| panic!("the inventory does not list {label}"))
    }

    /// Boot the idle unit out through the product and remove its file, and
    /// prove both; a unit never bootstrapped reads `absent` on both steps.
    pub(crate) fn cleanup_idle(&self) -> Result<(), String> {
        let bootout = self
            .command(&[
                "service",
                "bootout",
                &self.idle_label,
                "--host",
                HOST,
                "--domain",
                "user",
                "--json",
            ])
            .output()
            .map_err(|error| format!("idle unit bootout did not run: {error}"))?;
        let state: Option<serde_json::Value> = serde_json::from_slice(&bootout.stdout).ok();
        let state = state.as_ref().and_then(|report| report["state"].as_str());
        if !bootout.status.success() || !matches!(state, Some("booted_out" | "absent")) {
            return Err(format!(
                "idle unit bootout did not prove cleanup: {}",
                said(&bootout)
            ));
        }
        if self.idle_is_loaded() {
            return Err(format!(
                "launchd still holds {} after its bootout",
                self.idle_label
            ));
        }
        let plist = self.idle_plist.to_string_lossy().into_owned();
        let remove = self
            .command(&["space", "file", "remove", HOST, &plist, "--json"])
            .output()
            .map_err(|error| format!("idle unit file remove did not run: {error}"))?;
        let status: Option<serde_json::Value> = serde_json::from_slice(&remove.stdout).ok();
        let status = status.as_ref().and_then(|report| report["status"].as_str());
        if !remove.status.success()
            || !matches!(status, Some("removed" | "absent"))
            || self.idle_plist.exists()
        {
            return Err(format!(
                "idle unit file remove did not prove cleanup: {}",
                said(&remove)
            ));
        }
        Ok(())
    }
}
