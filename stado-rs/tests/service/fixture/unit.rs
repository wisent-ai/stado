//! One real launchd unit per case, and never one the operator runs.
//!
//! Every label is `com.wisent.stado-test.service-<pid>-<case>`: unique to
//! this test process and this case, under the prefix the product's own unit
//! enumeration accepts, and matching nothing in the operator's fleet. The
//! domain is this login's `gui/<uid>` — the session already running the test,
//! which needs no privilege at all (`launchctl bootstrap gui/<uid> <path>`
//! loads a plist from any path this account can read, measured on this host).
//!
//! [`Unit`] is a guard, not a helper: its `Drop` boots the label out of
//! launchd whether the case passed, failed or panicked, and every removal a
//! case asserts is read back off launchd rather than assumed. Nothing here
//! shadows a system tool — `/bin/launchctl` is addressed by absolute path.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Duration;

use super::fleet::{said, Fleet};

/// Bounded waiting for launchd to settle a label: attempts and the interval
/// between them.
const ATTEMPTS: usize = 300;
const INTERVAL: Duration = Duration::from_millis(100);

/// The program every unit in this area runs: a real system executable that
/// stays up long enough for a case to read launchd's answer about it.
pub const PROGRAM: &str = "/bin/sleep";
/// How long it stays up. Longer than any case needs, short enough that an
/// aborted run leaves nothing running for long.
pub const HOLD_SECONDS: &str = "600";

/// The launchd label the product mints for `service ensure NAME`, so a case
/// can address the unit the product created.
pub fn product_label(name: &str) -> String {
    format!(
        "{}compute.service.{name}",
        stado::deploy::local_install::FLEET_LABEL_PREFIX
    )
}

/// A service name unique to this process and this case.
pub fn service_name(case: &str) -> String {
    format!("stado-test-service-{}-{case}", std::process::id())
}

/// A launchd label unique to this process and this case, for a unit this
/// area loads itself and then asks the product to adopt.
pub fn adopted_label(case: &str) -> String {
    format!(
        "{}stado-test.service-{}-{case}",
        stado::deploy::local_install::FLEET_LABEL_PREFIX,
        std::process::id()
    )
}

/// One launchd label this case owns, in this login's own domain.
pub struct Unit {
    pub label: String,
    pub domain: String,
    pub plist: PathBuf,
}

impl Drop for Unit {
    fn drop(&mut self) {
        // Unconditional: a case that panicked before its own removal must not
        // leave a job in launchd. `bootout` of an absent label answers
        // `No such process`, which is the state this wants.
        launchctl(&["bootout", &self.qualified()]);
    }
}

impl Unit {
    /// Claim the label the product will install `name` under. Nothing is
    /// loaded yet; the guard exists first so the unit cannot outlive the case
    /// that creates it.
    pub fn claim(fleet: &Fleet, name: &str) -> Self {
        let label = product_label(name);
        Self {
            plist: fleet.agents().join(format!("{label}.plist")),
            label,
            domain: gui_domain(),
        }
    }

    /// Write a unit file and load it through this machine's real launchd:
    /// the pre-existing unit `service adopt` claims.
    pub fn loaded(fleet: &Fleet, label: &str) -> Self {
        let plist = fleet.agents().join(format!("{label}.plist"));
        std::fs::write(&plist, plist_text(label)).expect("write the unit file");
        let unit = Self {
            label: label.to_string(),
            domain: gui_domain(),
            plist,
        };
        // `enable` first: a label this login has ever disabled is refused with
        // `Bootstrap failed: 5: Input/output error`, which is not the state
        // any case here is about.
        launchctl(&["enable", &unit.qualified()]);
        let bootstrap = launchctl(&["bootstrap", &unit.domain, &unit.plist_arg()]);
        assert!(
            bootstrap.status.success(),
            "launchctl bootstrap {} refused: {}",
            unit.domain,
            said(&bootstrap)
        );
        unit.await_pid();
        unit
    }

    /// `gui/<uid>/<label>`, the way launchd is addressed.
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.domain, self.label)
    }

    /// The plist path as the product was told it.
    pub fn plist_arg(&self) -> String {
        self.plist.display().to_string()
    }

    /// The same file as launchd prints it back: macOS canonicalises a
    /// temporary directory, so a plist written under `/var/folders/...` is
    /// reported under `/private/var/...`.
    pub fn printed_path(&self) -> String {
        self.plist
            .canonicalize()
            .unwrap_or_else(|_| self.plist.clone())
            .display()
            .to_string()
    }

    /// What `launchctl print` says about this label, or `None` once launchd
    /// holds no job under it.
    pub fn printed(&self) -> Option<String> {
        let output = launchctl(&["print", &self.qualified()]);
        output.status.success().then(|| said(&output))
    }

    /// The pid launchd currently holds, read off `launchctl list <label>`.
    pub fn live_pid(&self) -> Option<u32> {
        let output = launchctl(&["list", &self.label]);
        if !output.status.success() {
            return None;
        }
        said(&output)
            .lines()
            .find_map(|line| line.trim().strip_prefix("\"PID\" = "))
            .and_then(|value| value.trim_end_matches(';').trim().parse().ok())
    }

    /// launchd's own answer once it holds nothing under this label, waited
    /// for rather than assumed. This is the sentence a removal is proved by.
    pub fn absence(&self) -> String {
        for _ in 0..ATTEMPTS {
            let output = launchctl(&["print", &self.qualified()]);
            if !output.status.success() {
                return said(&output);
            }
            std::thread::sleep(INTERVAL);
        }
        panic!("launchd still holds {}", self.qualified());
    }

    /// Wait until launchd reports a pid for this label, and return it.
    pub fn await_pid(&self) -> u32 {
        for _ in 0..ATTEMPTS {
            if let Some(pid) = self.live_pid() {
                return pid;
            }
            std::thread::sleep(INTERVAL);
        }
        panic!("{} never reported a pid", self.qualified());
    }
}

/// A minimal, real LaunchAgent: a label, a program that stays up, and a
/// declaration launchd will keep alive so a case can read a live pid.
fn plist_text(label: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{PROGRAM}</string>
    <string>{HOLD_SECONDS}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
"#
    )
}

/// This login's own launchd agent domain, from the real uid.
pub fn gui_domain() -> String {
    format!("gui/{}", uid())
}

/// The uid this test runs as, read from the kernel rather than an
/// environment variable.
pub fn uid() -> u32 {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .expect("/usr/bin/id did not run");
    assert!(output.status.success(), "/usr/bin/id -u failed");
    String::from_utf8(output.stdout)
        .expect("a uid is UTF-8")
        .trim()
        .parse()
        .expect("a numeric uid")
}

/// This machine's real `/bin/launchctl`. Nothing is substituted on PATH: the
/// binary is named by absolute path.
pub fn launchctl(args: &[&str]) -> Output {
    Command::new("/bin/launchctl")
        .args(args)
        .output()
        .expect("/bin/launchctl did not run")
}
