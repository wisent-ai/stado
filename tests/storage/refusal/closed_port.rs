//! A command whose object API refuses the connection ends with that refusal,
//! having sent the request once; it does not print its failed wait and then
//! send the same request into the same refusal.
//!
//! The real `stado storage get` is pointed at a loopback port this machine
//! handed out and nothing listens on, which is what a command saw from an
//! object API that was not running: its `blad czekania` line said
//! `connection refused` and the command kept going.
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const URI: &str = "stado://probe/refusal/closed.bin";

struct Run {
    root: PathBuf,
}

impl Run {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("storage-refusal");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(std::process::id().to_string());
        std::fs::create_dir_all(&root).expect("create the run directory");
        Self { root }
    }

    /// The real stado with nothing of this account's configuration.
    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH is set"))
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// A loopback address on a port this machine hands out now and nothing
    /// binds.
    fn closed_address(&self) -> String {
        let output = self
            .stado()
            .args(["host", "local", "port"])
            .output()
            .expect("run the real stado");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            output.status.success(),
            "stado host local port failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        format!("{}:{}", Ipv4Addr::LOCALHOST, printed.trim())
    }

    fn get(&self, api: &str) -> Output {
        let output = self
            .stado()
            .args(["storage", "get", URI, "-"])
            .env("STADO_API_URL", format!("http://{api}"))
            .env("STADO_API_TOKEN", "refusal-probe")
            .output()
            .expect("run the real stado");
        std::fs::write(self.root.join("get.stderr"), &output.stderr)
            .expect("keep the command's stderr beside the run");
        output
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_refused_connection_ends_the_command_with_the_refusal_after_one_request() {
    let run = Run::new();
    let api = run.closed_address();
    let output = run.get(&api);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        !output.status.success(),
        "a read the object API refused does not succeed: {said}"
    );
    assert!(
        said.contains("the object API refused the object GET") && said.contains(&api),
        "the command's own error names the refusal and where it came from: {said}"
    );
    let requests: Vec<&str> = said
        .lines()
        .filter(|line| line.starts_with("czekam: GET /api/object"))
        .collect();
    assert!(
        matches!(requests.as_slice(), [_]),
        "the refused GET was sent once, not again into the same refusal: {requests:?}"
    );
    assert!(
        said.lines()
            .any(|line| line.starts_with("blad czekania: GET /api/object")),
        "the failed wait is written before the command ends: {said}"
    );
}
