//! A launchd unit this area owns: really loaded, really running, really
//! restarted, really removed.
//!
//! `/bin/launchctl bootstrap gui/<uid>` needs no privilege this test does not
//! have — it loads an agent into the login session already running the test —
//! and every label carries this process's own pid, so nothing here can
//! address a unit the operator's fleet holds. The unit is booted out in
//! `Drop`, including while a case is panicking, and the removal is read back
//! off launchd rather than assumed.
//!
//! The image is a real file: a copy of this test binary, which is the only
//! executable the area can place inside its own tempdir (macOS SIGKILLs a
//! copy of a signed platform binary). The replacement is that same binary
//! with a marker appended — a different inode, a different size and a
//! different sha256, still executable, because a Mach-O code directory
//! covers the bytes it was signed over and not what follows them. That is
//! what makes a real `launchctl kickstart` able to land on it and hold.

use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use stado::deploy::service::{running_images, ImageIdentity};

use crate::host::{digest, Host};

/// The variable the probe body reads. Set only in the unit's own
/// `EnvironmentVariables`, never in the parent test process.
pub const HOLD: &str = "STADO_REFRESH_IMAGE_HOLD_SECONDS";

/// How long the held process keeps its image. Longer than any case here
/// needs, short enough that an aborted run leaves nothing for long.
pub const HOLD_SECONDS: u64 = 180;

/// The name of the probe body inside the copied binary.
const PROBE: &str = "refresh_image_probe_child";

/// Bounded waiting for launchd to start the job and for the kernel to report
/// its image: attempts and the interval between them.
const ATTEMPTS: usize = 300;
const INTERVAL: Duration = Duration::from_millis(100);

/// What makes the replacement a different file from the one being executed.
const MARKER: &[u8] = b"\n# stado refresh-unit-image replacement marker\n";

/// One real launchd agent, its image, and the digest of the bytes it started
/// on.
pub struct LoadedUnit {
    pub label: String,
    pub plist: PathBuf,
    /// The file the unit's `ProgramArguments` name.
    pub program: PathBuf,
    /// `gui/<uid>` for this login.
    pub domain: String,
    pub pid: u32,
    /// sha256 of the bytes the process is executing, computed by this test
    /// before anything replaced them.
    pub digest: String,
    loaded: bool,
}

impl Drop for LoadedUnit {
    fn drop(&mut self) {
        if self.loaded {
            launchctl(&["bootout", &self.qualified()]);
            self.loaded = false;
        }
    }
}

impl LoadedUnit {
    /// Load an agent whose program is a copy of this test binary, and return
    /// once the kernel reports that copy as the image its process executes.
    pub fn start(host: &Host, nickname: &str) -> Self {
        let program = host.root.join("bin").join(nickname);
        std::fs::copy(current_exe(), &program).expect("copy the test binary into the fixture");
        let label = label(nickname);
        let plist = write_plist(
            host,
            &label,
            &[
                program.display().to_string(),
                "--exact".to_string(),
                PROBE.to_string(),
            ],
            true,
        );
        let mut unit = Self {
            digest: digest(&program),
            label,
            plist,
            program,
            domain: gui_domain(host),
            pid: 0,
            loaded: false,
        };
        let bootstrap = launchctl(&["bootstrap", &unit.domain, &unit.plist.display().to_string()]);
        assert!(
            bootstrap.status.success(),
            "launchctl bootstrap {} refused: {}",
            unit.domain,
            text(&bootstrap)
        );
        unit.loaded = true;
        unit.pid = unit.await_image();
        unit
    }

    /// `gui/<uid>/<label>`, the way launchd is addressed.
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.domain, self.label)
    }

    /// The image the live process is executing, through the product's own
    /// reader.
    pub fn image(&self) -> ImageIdentity {
        running_images(&[self.pid])
            .expect("the kernel image reader answered")
            .remove(&self.pid)
            .unwrap_or_else(|| panic!("pid {} reports no executing image", self.pid))
    }

    /// The pid launchd currently holds for this label, read off `launchctl
    /// list`. `None` once the job is not running.
    pub fn live_pid(&self) -> Option<u32> {
        let output = launchctl(&["list", &self.label]);
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.trim().strip_prefix("\"PID\" = "))
            .and_then(|value| value.trim_end_matches(';').trim().parse().ok())
    }

    /// Unlink the running image and put a different real executable at the
    /// same path, exactly as an installer replaces one. Returns the sha256 of
    /// the replacement, computed here.
    pub fn replace_image(&self) -> String {
        std::fs::remove_file(&self.program).expect("unlink the running image");
        std::fs::copy(current_exe(), &self.program).expect("write the replacement");
        std::fs::File::options()
            .append(true)
            .open(&self.program)
            .expect("open the replacement to mark it")
            .write_all(MARKER)
            .expect("mark the replacement");
        digest(&self.program)
    }

    /// Push the declared file's modification time back by `seconds`, which is
    /// how the settle window is crossed rather than waited out.
    pub fn backdate(&self, seconds: i64) {
        let when = SystemTime::now()
            - Duration::from_secs(u64::try_from(seconds).expect("a positive age in seconds"));
        std::fs::File::options()
            .write(true)
            .open(&self.program)
            .expect("open the declared file to retime it")
            .set_times(std::fs::FileTimes::new().set_modified(when))
            .expect("retime the declared file");
    }

    /// Boot the unit out and return what launchd says about the label
    /// afterwards — the removal, read off the system rather than assumed.
    pub fn remove(&mut self) -> String {
        let bootout = launchctl(&["bootout", &self.qualified()]);
        self.loaded = false;
        assert!(
            bootout.status.success(),
            "launchctl bootout {} refused: {}",
            self.qualified(),
            text(&bootout)
        );
        for _ in 0..ATTEMPTS {
            let print = launchctl(&["print", &self.qualified()]);
            if !print.status.success() {
                return text(&print);
            }
            std::thread::sleep(INTERVAL);
        }
        panic!("launchd still holds {} after bootout", self.qualified());
    }

    /// Wait until launchd reports a pid whose executing image is the copy
    /// this unit was loaded with, and return that pid.
    fn await_image(&self) -> u32 {
        let inode = std::fs::metadata(&self.program)
            .expect("the program is on disk")
            .ino();
        for _ in 0..ATTEMPTS {
            if let Some(pid) = self.live_pid() {
                if running_images(&[pid])
                    .is_ok_and(|images| images.get(&pid).is_some_and(|it| it.inode == inode))
                {
                    return pid;
                }
            }
            std::thread::sleep(INTERVAL);
        }
        panic!(
            "{} never reported inode {inode} at {} as its executing image",
            self.qualified(),
            self.program.display()
        );
    }
}

/// A label unique to this process and this case, under the prefix the
/// product's unit enumeration accepts.
pub fn label(nickname: &str) -> String {
    format!(
        "{}stado-test.refresh-image-{}-{nickname}",
        stado::deploy::local_install::FLEET_LABEL_PREFIX,
        std::process::id()
    )
}

/// Write a unit file into the fixture's agent directory. `argv` empty means a
/// declaration that names no program at all.
pub fn write_plist(host: &Host, label: &str, argv: &[String], hold: bool) -> PathBuf {
    let arguments = if argv.is_empty() {
        String::new()
    } else {
        let entries = argv
            .iter()
            .map(|word| format!("    <string>{word}</string>"))
            .collect::<Vec<String>>()
            .join("\n");
        format!("  <key>ProgramArguments</key>\n  <array>\n{entries}\n  </array>\n")
    };
    let environment = if hold {
        format!(
            "  <key>EnvironmentVariables</key>\n  <dict><key>{HOLD}</key><string>{HOLD_SECONDS}\
             </string></dict>\n"
        )
    } else {
        String::new()
    };
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{label}</string>
  <key>RunAtLoad</key><true/>
{arguments}{environment}</dict>
</plist>
"#
    );
    let path = host.agents().join(format!("{label}.plist"));
    std::fs::write(&path, body).expect("write the unit file");
    path
}

/// This test binary's own path: the executable every image here is a copy of.
pub fn current_exe() -> PathBuf {
    std::env::current_exe().expect("this test binary has a path")
}

/// This login's own agent domain, from the uid owning the fixture's home.
fn gui_domain(host: &Host) -> String {
    let uid = std::fs::metadata(&host.home)
        .expect("the fixture home is readable")
        .uid();
    format!("gui/{uid}")
}

/// This machine's real `/bin/launchctl`.
fn launchctl(args: &[&str]) -> Output {
    Command::new("/bin/launchctl")
        .args(args)
        .output()
        .expect("/bin/launchctl did not run")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
