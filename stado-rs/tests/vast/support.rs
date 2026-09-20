//! The isolated machine the Vast stories drive: an empty local queue store,
//! a home with no Skarbiec bearer in it, and the real binary with every
//! command's output retained beside the revision it ran at.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

/// How long the daemon story lets the loop run before reading its decisions.
/// The loop polls once a second, and a debug build spends about a second
/// starting, so six seconds is the startup line, the listing decision and
/// several polls after it.
pub(crate) const DAEMON_OBSERVATION_SECONDS: u64 = 6;

pub(crate) struct Bridge {
    pub(crate) home: PathBuf,
    pub(crate) storage: PathBuf,
}

impl Bridge {
    /// A machine holding no credential of any kind: `HOME` is a fresh
    /// directory, so the control-plane bearer and the agent grant are both
    /// absent and nothing in these stories can reach the operator's vault.
    pub(crate) fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/vast-runs");
        fs::create_dir_all(&root).unwrap();
        let home = tempfile::Builder::new()
            .prefix("vast-")
            .tempdir_in(root)
            .unwrap()
            .keep();
        let storage = home.join("store");
        fs::create_dir_all(storage.join("queue")).unwrap();
        fs::create_dir_all(storage.join("running")).unwrap();
        fs::create_dir_all(storage.join("capacity")).unwrap();
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        fs::write(home.join("revision.txt"), revision.stdout).unwrap();
        Self { home, storage }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("STADO_CONFIG", self.home.join("no-config.json"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", &self.storage)
            .env("WC_PROVIDERS", "local")
            .env("WC_VAST_AUTO_LIST", "false");
        command
    }

    pub(crate) fn invoke(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        self.retain(args, &output.stdout, &output.stderr, output.status.code());
        output
    }

    /// Run the daemon for a fixed window and answer with what it printed and
    /// whether it was still looping when the window closed. The exit status
    /// belongs to the kill, so no story asserts on it.
    ///
    /// Output goes straight to the evidence files rather than through pipes:
    /// a killed child's pipe came back empty here while the same command
    /// printed four decisions when run by hand, and evidence that survives
    /// the kill is the point of retaining it.
    pub(crate) fn observe_daemon(&self, args: &[&str], seconds: u64) -> (String, String, bool) {
        let evidence = self.home.join("evidence");
        fs::create_dir_all(&evidence).unwrap();
        let name = args.join("_").replace('/', "_").replace("--", "");
        let out_path = evidence.join(format!("{name}.stdout"));
        let err_path = evidence.join(format!("{name}.stderr"));
        let mut child: Child = self
            .command()
            .args(args)
            .stdout(Stdio::from(fs::File::create(&out_path).unwrap()))
            .stderr(Stdio::from(fs::File::create(&err_path).unwrap()))
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        let alive = child.try_wait().unwrap().is_none();
        let _ = child.kill();
        let status = child.wait().unwrap();
        fs::write(
            evidence.join(format!("{name}.exit")),
            format!("{:?}", status.code()),
        )
        .unwrap();
        (
            fs::read_to_string(&out_path).unwrap(),
            fs::read_to_string(&err_path).unwrap(),
            alive,
        )
    }

    fn retain(&self, args: &[&str], stdout: &[u8], stderr: &[u8], code: Option<i32>) {
        let name = args.join("_").replace('/', "_").replace("--", "");
        let evidence = self.home.join("evidence");
        fs::create_dir_all(&evidence).unwrap();
        fs::write(evidence.join(format!("{name}.stdout")), stdout).unwrap();
        fs::write(evidence.join(format!("{name}.stderr")), stderr).unwrap();
        fs::write(evidence.join(format!("{name}.exit")), format!("{code:?}")).unwrap();
    }
}
