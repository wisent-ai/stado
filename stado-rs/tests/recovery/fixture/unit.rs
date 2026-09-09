//! One real launchd unit per case, in this login's own domain, and never one
//! the operator runs.
//!
//! The domain is `gui/<uid>` — the session already running the test, which
//! needs no privilege at all: `launchctl bootstrap gui/<uid> <path>` loads a
//! plist from any path this account can read, measured on this host.
//!
//! [`Unit`] is a guard, not a helper. [`BEACON`] is a fixed label inside the
//! product (`host_recovery::MANAGED_AGENTS`), so a case cannot make it unique
//! per process the way the service area does; `Drop` is what guarantees no
//! `com.wisent.*` label this area loads is left in launchd, whether the case
//! passed, failed or panicked. Every removal a case asserts is read back off
//! launchd rather than assumed.
//!
//! Nothing here shadows a system tool: `/bin/launchctl` and `/usr/bin/id` are
//! addressed by absolute path.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use super::fleet::{said, Fleet};

/// The beacon's label is fixed inside the product, so two cases about it
/// would address one launchd job. Cargo runs the cases of a test binary on
/// threads of one process, so a mutex is what makes them take turns.
static BEACON_LABEL_LOCK: Mutex<()> = Mutex::new(());

/// Hold the beacon label for the rest of a case. Taken before the unit
/// guard, so it is released after the label has been booted out. A case that
/// panicked poisons nothing worth refusing a turn over: the lock guards a
/// name, and the guard that clears it runs regardless.
pub fn beacon_lock() -> MutexGuard<'static, ()> {
    BEACON_LABEL_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Bounded waiting for launchd to settle a label: attempts and the interval
/// between them.
const ATTEMPTS: usize = 300;
const INTERVAL: Duration = Duration::from_millis(100);

/// The launchd label `host_recovery::MANAGED_AGENTS` reloads on every pass.
/// A fixed name in the product, not a choice this test makes.
pub const BEACON: &str = "com.wisent.host-health-beacon";

/// The plist path that same fixed list carries for a host which declares no
/// unit of its own. `$HOME` reaches the host unexpanded and is expanded
/// there, so with an isolated `HOME` it names a file in the case's tempdir —
/// and it is the literal string the pass reports a missing file under.
pub const BEACON_DECLARED_PATH: &str =
    "$HOME/Library/LaunchAgents/com.wisent.host-health-beacon.plist";

/// The program every unit here runs: a real system executable that stays up
/// long enough for a case to read launchd's answer about it.
pub const PROGRAM: &str = "/bin/sleep";
/// How long it stays up. Longer than any case needs, short enough that an
/// aborted run leaves nothing running for long.
pub const HOLD_SECONDS: &str = "600";

/// The scoped health configuration the recovery pass demands of the beacon
/// before it will load it, key for key as the pass reads them with
/// `plutil -extract`. The consumer is the one value it compares rather than
/// merely requires.
pub const BEACON_CONSUMER_KEY: &str = "STADO_HOST_HEALTH_SKARBIEC_CONSUMER";
pub const BEACON_CONSUMER: &str = "stado-host-health-beacon";
/// A credential the beacon must NOT carry: an ambient Google credential is
/// authority the pass refuses to load a unit with.
pub const AMBIENT_CREDENTIAL_KEY: &str = "GOOGLE_APPLICATION_CREDENTIALS";

/// The whole scoped configuration a beacon the pass accepts must declare.
pub fn beacon_environment() -> Vec<(&'static str, &'static str)> {
    vec![
        ("STADO_HOST_HEALTH_API_URL", "http://127.0.0.1:8765"),
        (
            "STADO_HOST_HEALTH_SKARBIEC_URL",
            "https://skarbiec.wisent.com",
        ),
        (BEACON_CONSUMER_KEY, BEACON_CONSUMER),
        ("STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE", "/dev/null"),
        ("STADO_BIN", PROGRAM),
    ]
}

/// One launchd label a case owns, in this login's own domain.
pub struct Unit {
    pub label: String,
    pub domain: String,
    pub plist: PathBuf,
}

impl Drop for Unit {
    fn drop(&mut self) {
        // Unconditional: a case that panicked before its own readback must
        // not leave a job in launchd. `bootout` of an absent label answers
        // `No such process`, which is the state this wants.
        launchctl(&["bootout", &self.qualified()]);
    }
}

impl Unit {
    /// Claim a label under this fleet's isolated agent directory. Nothing is
    /// loaded yet; the guard exists first so the unit cannot outlive the case.
    pub fn claim(fleet: &Fleet, label: &str) -> Self {
        Self {
            plist: fleet.agents().join(format!("{label}.plist")),
            label: label.to_string(),
            domain: gui_domain(),
        }
    }

    /// `gui/<uid>/<label>`, the way launchd is addressed.
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.domain, self.label)
    }

    pub fn plist_arg(&self) -> String {
        self.plist.display().to_string()
    }

    /// Write this unit's file: a real LaunchAgent with no declared
    /// environment, which is every unit here except the beacon.
    pub fn write_plist(&self) {
        std::fs::write(&self.plist, plist_text(&self.label, &[])).expect("write the unit file");
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

    /// Load this unit through this machine's real launchd and wait until it
    /// has a pid, so a case that stops it is stopping something.
    pub fn load(&self) -> u32 {
        // `enable` first: a label this login has ever disabled is refused
        // with `Bootstrap failed: 5: Input/output error`, which is not the
        // state any case here is about.
        launchctl(&["enable", &self.qualified()]);
        let bootstrap = launchctl(&["bootstrap", &self.domain, &self.plist_arg()]);
        assert!(
            bootstrap.status.success(),
            "launchctl bootstrap {} refused: {}",
            self.domain,
            said(&bootstrap)
        );
        self.await_pid()
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
}

/// A minimal, real LaunchAgent: a label, a program that stays up, a
/// declaration launchd will keep alive so a case can read a live pid, and the
/// declared environment the case is about.
pub fn plist_text(label: &str, environment: &[(&str, &str)]) -> String {
    let mut env_block = String::new();
    if !environment.is_empty() {
        env_block.push_str("  <key>EnvironmentVariables</key>\n  <dict>\n");
        for (name, value) in environment {
            env_block.push_str(&format!("    <key>{name}</key><string>{value}</string>\n"));
        }
        env_block.push_str("  </dict>\n");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         \x20 <key>Label</key><string>{label}</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n\
         \x20   <string>{PROGRAM}</string>\n    <string>{HOLD_SECONDS}</string>\n  </array>\n\
         \x20 <key>KeepAlive</key><true/>\n\
         {env_block}</dict>\n</plist>\n"
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
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("a uid is a number")
}

/// This machine's real `/bin/launchctl`. Nothing is substituted on PATH: the
/// binary is named by absolute path.
pub fn launchctl(args: &[&str]) -> Output {
    Command::new("/bin/launchctl")
        .args(args)
        .output()
        .expect("/bin/launchctl did not run")
}
