//! A launchd unit this test owns, in this user's own domain.
//!
//! The area this replaces drove a scripted `launchctl` behind a scripted
//! `ssh`, so nothing it asserted had ever been true of a running unit. Here the
//! test writes its own plist, bootstraps it into `gui/<uid>` under a label
//! nobody else uses, and boots it out again on the way down — including when a
//! case panics, because `Drop` runs either way.
//!
//! Nothing outside the test is touched: the plist lives in the test's tempdir,
//! the label carries this process's id, and the program is `/bin/sleep`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// How long the unit's program sleeps. Long enough that the read happens while
/// it runs, short enough that a leaked process leaves on its own.
const SLEEP_SECONDS: &str = "600";

pub fn uid() -> String {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .expect("the operating system reports this user id");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

pub fn launchctl(args: &[&str]) -> Output {
    Command::new("/bin/launchctl")
        .args(args)
        .output()
        .expect("launchctl runs")
}

/// A unit that exists for one case, named after the process that made it.
pub struct OwnedUnit {
    pub label: String,
    pub plist: PathBuf,
    booted: bool,
}

impl OwnedUnit {
    pub fn declared(directory: &Path) -> Self {
        let label = format!("com.wisent.test.stado-domain-{}", std::process::id());
        let plist = directory.join(format!("{label}.plist"));
        let document = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>{label}</string>\n\
             <key>ProgramArguments</key><array><string>/bin/sleep</string>\
             <string>{SLEEP_SECONDS}</string></array>\n\
             <key>RunAtLoad</key><true/>\n\
             </dict></plist>\n"
        );
        std::fs::write(&plist, document).expect("write the unit's own plist");
        Self {
            label,
            plist,
            booted: false,
        }
    }

    pub fn service_target(&self) -> String {
        format!("gui/{}/{}", uid(), self.label)
    }

    /// Load it for real, and report what launchd itself says about it.
    pub fn bootstrap(&mut self) -> Output {
        let domain = format!("gui/{}", uid());
        let output = launchctl(&["bootstrap", &domain, &self.plist.to_string_lossy()]);
        assert!(
            output.status.success(),
            "launchd refused the test's own unit: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.booted = true;
        output
    }

    pub fn printed(&self) -> Output {
        launchctl(&["print", &self.service_target()])
    }

    /// What launchd reports for one field of the loaded unit, so the product's
    /// claim can be compared against it.
    pub fn printed_field(&self, name: &str) -> Option<String> {
        let printed = self.printed();
        if !printed.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&printed.stdout).into_owned();
        text.lines()
            .map(str::trim)
            .find(|line| line.starts_with(&format!("{name} = ")))
            .map(|line| line[name.len() + " = ".len()..].trim().to_string())
    }

    pub fn is_loaded(&self) -> bool {
        self.printed().status.success()
    }

    pub fn forget(&mut self) {
        self.booted = false;
    }
}

impl Drop for OwnedUnit {
    fn drop(&mut self) {
        if !self.booted {
            return;
        }
        let _ = launchctl(&["bootout", &self.service_target()]);
    }
}
