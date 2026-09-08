use chrono::TimeDelta;

use crate::deploy::service::*;

/// One managed unit whose live process is not executing the file the unit's
/// own `ProgramArguments` name — or one the question could not be asked about.
///
/// Measured on `lukasz-macbook` on 2026-09-02, and the measurement is why this
/// exists. `com.wisent.transcript-lake-stream` had been running pid 99986
/// since the previous afternoon on inode 125374164, 3,058,288 bytes, zero
/// links; the `/Users/lukaszbartoszcze/.local/bin/transcript-lake` its plist
/// names resolved to inode 181713431 at 2,958,720 bytes, written that morning.
/// Nothing in the fleet reported it and nothing could have:
/// `self_update::recycle_replaced_units` cycles a unit only as a side effect
/// of the invocation that replaced its bytes, matches by string equality on
/// `argv[0]`, compares no identity at all, and never revisits a unit it missed
/// or that a failed `kickstart` left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleUnitImage {
    pub host: String,
    /// launchd label. Empty on the row that reports a whole host unread,
    /// which is about no single unit.
    pub unit: String,
    /// The unit file the declaration was read from.
    pub unit_path: String,
    /// `ProgramArguments[0]`: the file the unit says it starts.
    pub program: String,
    pub pid: Option<u32>,
    /// How long the process has been alive.
    pub process_age_seconds: Option<i64>,
    /// How long ago the declared program was last written.
    pub installed_age_seconds: Option<i64>,
    pub state: ImageState,
}

impl StaleUnitImage {
    /// Stable machine-readable category, in `registry doctor`'s vocabulary.
    pub fn kind(&self) -> &'static str {
        match self.state {
            ImageState::Unlinked { .. } | ImageState::Replaced { .. } => "stale-unit-image",
            ImageState::Unread { .. } => "unread-unit-image",
        }
    }

    /// An age in the spelling every other `registry doctor` row uses.
    fn age(seconds: Option<i64>) -> String {
        seconds.map_or_else(
            || "an unread time".to_string(),
            |seconds| crate::cli::registry::human_age(TimeDelta::seconds(seconds)),
        )
    }

    /// The row an operator reads. It names the unit, the path and BOTH
    /// identities, because "stale" on its own sends somebody back to `lsof` to
    /// re-derive the two facts the check already holds.
    pub fn sentence(&self) -> String {
        let pid = self
            .pid
            .map_or_else(|| "no pid".to_string(), |pid| format!("pid {pid}"));
        match &self.state {
            ImageState::Unlinked { running, installed } => format!(
                "{} is running {pid}, started {} ago, and the executable that process is running \
                 has been unlinked: {}. Its ProgramArguments name {}, which now holds {}, written \
                 {} ago. No copy of the running build is left on disk, so the process is serving \
                 bytes nothing on this host can reproduce. Restarting the unit is what puts it on \
                 the installed file, and nothing does that on its own: \
                 self_update::recycle_replaced_units cycles units only inside the invocation that \
                 replaced them and never revisits one it missed",
                self.unit,
                Self::age(self.process_age_seconds),
                running.describe(),
                self.program,
                installed.describe(),
                Self::age(self.installed_age_seconds),
            ),
            ImageState::Replaced { running, installed } => format!(
                "{} is running {pid}, started {} ago, and the executable that process is running \
                 is not the file its unit declares: it is {} at {}. Its ProgramArguments name {}, \
                 which holds {}, written {} ago. Both files still exist, so the running one can be \
                 compared against the installed one before the unit is restarted",
                self.unit,
                Self::age(self.process_age_seconds),
                running.describe(),
                running.path,
                self.program,
                installed.describe(),
                Self::age(self.installed_age_seconds),
            ),
            ImageState::Unread { subject, reason } => format!(
                "{subject} could not be read, so whether the live process is executing the file \
                 its unit declares is unknown here and is NOT reported as agreement: {reason}"
            ),
        }
    }

    pub fn to_json(&self) -> Value {
        let identity = |image: &ImageIdentity| {
            json!({
                "path": image.path,
                "device": image.device,
                "inode": image.inode,
                "bytes": image.bytes,
                "links": image.links,
            })
        };
        let (state, running, installed, unread) = match &self.state {
            ImageState::Unlinked { running, installed } => (
                "unlinked",
                Some(identity(running)),
                Some(identity(installed)),
                None,
            ),
            ImageState::Replaced { running, installed } => (
                "replaced",
                Some(identity(running)),
                Some(identity(installed)),
                None,
            ),
            ImageState::Unread { subject, reason } => {
                ("unread", None, None, Some(format!("{subject}: {reason}")))
            }
        };
        json!({
            "host": self.host,
            "unit": self.unit,
            "unit_path": self.unit_path,
            "program": self.program,
            "pid": self.pid,
            "process_age_seconds": self.process_age_seconds,
            "installed_age_seconds": self.installed_age_seconds,
            "state": state,
            "running": running,
            "installed": installed,
            "unread_reason": unread,
        })
    }
}

/// Resolve Apple's `/bin/sh` dispatcher without duplicating its shell-selection
/// policy. Privileged mode ignores user startup files; the readiness byte keeps
/// the selected shell alive while the ordinary kernel image reader observes it.
#[cfg(target_os = "macos")]
pub(super) fn selected_macos_shell() -> Result<PathBuf, String> {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let mut child = Command::new("/bin/sh")
        .args(["-p", "-c", "printf R; read -r stado_image_continue"])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not resolve macOS /bin/sh: {error}"))?;
    let result = (|| {
        let mut ready = [0_u8; 1];
        child
            .stdout
            .as_mut()
            .ok_or_else(|| "macOS /bin/sh has no readiness pipe".to_string())?
            .read_exact(&mut ready)
            .map_err(|error| format!("macOS /bin/sh did not become observable: {error}"))?;
        if ready != *b"R" {
            return Err("macOS /bin/sh returned an unexpected readiness byte".to_string());
        }
        running_images(&[child.id()])?
            .remove(&child.id())
            .map(|image| PathBuf::from(image.path))
            .ok_or_else(|| "the shell selected by macOS /bin/sh has no readable image".to_string())
    })();
    // EOF releases the shell's read, including every failed observation.
    drop(child.stdin.take());
    child
        .wait()
        .map_err(|error| format!("could not reap the macOS shell image reader: {error}"))?;
    result
}
