//! `stado host run attach` tells the program's own answer from the end of
//! its channel. The wrapper reports the program's status the moment it ends,
//! so a program that exits while a child holds its output answers its own
//! status, the child is named, and each stage after the exit is a wait of its
//! own; the JSON receipt keeps the program's status beside the channel's; and
//! a channel that ends before the program reported is said as the channel's
//! failure, never as the program's answer. On Linux the read of who holds
//! the output is the `/proc/<pid>/fd` links, never lsof over the whole host.
//!
//! Each test is one isolated deployment under the build directory: its own
//! HOME (the managed run area and the signal markers live there), a local
//! store whose registry holds this machine as `marker-host`, and a program
//! placed in a run of its own. A child that holds the output blocks opening
//! a FIFO the test opens to release it, so no duration decides when it ends.
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const TARGET: &str = "marker-host";
const PROGRAM_LINE: &str = "program line";
const PROGRAM_EXITED: &str = "stado host run attach: program exited";
const WRAPPER_EXITING: &str = "stado host run attach: wrapper exiting";
const HOLDING: &str = "still hold its output, which is no longer relayed";
const READING: &str = "stado host run attach: reading which processes hold its output with";
const READ: &str = "stado host run attach: read which processes hold its output";
const PROC_READER: &str = "the /proc/<pid>/fd links naming its relays";
const LSOF_READER: &str = "over every open file on this host";
const PROGRAM_WAIT: &str = "the attached remote program to exit";
const HOLDERS_WAIT: &str = "which processes on marker-host still hold an attached program's output";
const CLOSING_WAIT: &str = "the host channel of an attached program to close after it exited";
const CHANNEL_FIRST: &str = "did not report its exit: the host channel to marker-host ended (";
const NOT_KNOWN: &str = "first, so whether it ran is not known";

struct Deployment {
    root: PathBuf,
    release: PathBuf,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let parent = repository.join(".build/host-channel-termin");
        std::fs::create_dir_all(&parent).unwrap();
        let root = parent
            .canonicalize()
            .unwrap()
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        let release = root.join("release");
        let made = Command::new("mkfifo").arg(&release).status().unwrap();
        assert!(made.success(), "the release FIFO is made at {release:?}");
        let deployment = Self { root, release };
        deployment.cli("config-init", &["config", "init"]);
        deployment.import_registry();
        deployment
    }

    fn run(&self, name: &str, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH names the tools"),
            )
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }

    fn cli(&self, name: &str, args: &[&str]) {
        let output = self.run(name, args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// The shared directory template with this machine as `marker-host`.
    fn import_registry(&self) {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(
            "registry-import",
            &["registry", "import", &registry.to_string_lossy()],
        );
    }

    /// `body` as an executable in a run of its own, after a line on stdout.
    fn place_program(&self, body: &str) -> String {
        let run = self
            .root
            .join(".stado/work/runs")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&run).unwrap();
        let program = run.join("program.sh");
        std::fs::write(
            &program,
            format!("#!/bin/bash\nprintf '%s\\n' '{PROGRAM_LINE}'\n{body}"),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&program).unwrap().permissions();
        permissions.set_mode(permissions.mode() | executable_bits());
        std::fs::set_permissions(&program, permissions).unwrap();
        program.to_string_lossy().into_owned()
    }

    /// A child holding the program's stdout and stderr until this test
    /// releases it, named on stderr.
    fn holding_child(&self) -> String {
        let release = self.release.to_string_lossy();
        format!("( read -r _ < '{release}' ) &\nprintf 'child %s\\n' \"$!\" >&2\n")
    }

    /// Let the child go: a writer opening the FIFO ends its blocked open, and
    /// closing it ends its read. Without a child waiting nothing opens it.
    fn release_child(&self) {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(nix::libc::O_NONBLOCK)
            .open(&self.release);
    }

    fn attach(&self, name: &str, program: &str, json: bool) -> Output {
        let mut arguments = vec!["host", "run", "attach", TARGET, "--program", program];
        if json {
            arguments.push("--json");
        }
        let output = self.run(name, &arguments);
        self.release_child();
        output
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        self.release_child();
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

/// The owner, group and other execute bits, as `chmod +x` adds them.
fn executable_bits() -> u32 {
    u32::from(nix::libc::S_IXUSR | nix::libc::S_IXGRP | nix::libc::S_IXOTH)
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

/// What `program` exits with on this machine.
fn status_of(program: &str) -> i32 {
    Command::new(program).status().unwrap().code().unwrap()
}

/// A wait on `what` opened and ended as done.
fn waited(said: &str, what: &str) -> bool {
    said.contains(&format!("czekam: {what}")) && said.contains(&format!("koniec czekania: {what}"))
}

#[test]
fn a_program_leaving_a_holder_answers_its_own_status_and_each_stage_is_a_wait() {
    let deployment = Deployment::start();
    let refused = status_of("false");
    let program = deployment.place_program(&format!("{}false\nexit\n", deployment.holding_child()));
    let output = deployment.attach("holder", &program, false);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(
        output.status.code(),
        Some(refused),
        "the program's own status: {said}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(PROGRAM_LINE),
        "the program's own output is relayed: {said}"
    );
    assert!(
        said.contains(&format!("{PROGRAM_EXITED} {refused}"))
            && said.contains(&format!("{WRAPPER_EXITING} {refused}")),
        "the wrapper reports the program's status and its own: {said}"
    );
    let reader = if cfg!(target_os = "linux") {
        PROC_READER
    } else {
        LSOF_READER
    };
    assert!(
        said.lines()
            .any(|line| line.starts_with(READING) && line.contains(reader)),
        "the read says what it reads with ({reader}): {said}"
    );
    if cfg!(target_os = "linux") {
        assert!(!said.contains("lsof"), "Linux never runs lsof: {said}");
    }
    assert!(said.contains(READ), "the read says it ended: {said}");
    let child = said
        .lines()
        .find_map(|line| line.strip_prefix("child "))
        .unwrap_or_else(|| panic!("the program named its child: {said}"));
    assert!(
        said.lines()
            .any(|line| line.contains(HOLDING) && line.contains(&format!("{child} ("))),
        "the child {child} is named as still holding the output: {said}"
    );
    for stage in [PROGRAM_WAIT, HOLDERS_WAIT, CLOSING_WAIT] {
        assert!(
            waited(&said, stage),
            "the stage '{stage}' is a wait opened and ended: {said}"
        );
    }
}

#[test]
fn the_receipt_keeps_the_program_status_beside_the_channels() {
    let deployment = Deployment::start();
    for (name, ending) in [("succeeds", "true"), ("refuses", "false")] {
        let status = status_of(ending);
        let program = deployment.place_program(&format!("printf 'said\\n' >&2\n{ending}\nexit\n"));
        let output = deployment.attach(name, &program, true);
        let said = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(
            output.status.code(),
            Some(status),
            "{name}: the program's status: {said}"
        );
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!("{name}: the receipt is one JSON document ({error}): {said}")
        });
        assert_eq!(receipt["program_exit"], json!(status), "{name}: {receipt}");
        assert_eq!(receipt["wrapper_exit"], json!(status), "{name}: {receipt}");
        assert_eq!(receipt["exit_code"], json!(status), "{name}: {receipt}");
        let expected = if status == status_of("true") {
            "exited"
        } else {
            "failed"
        };
        assert_eq!(receipt["status"], json!(expected), "{name}: {receipt}");
        let kept = receipt["stderr"].as_str().unwrap_or_default();
        assert!(
            kept.contains("said"),
            "{name}: the program's stderr is kept: {receipt}"
        );
        assert!(
            !kept.contains(PROGRAM_EXITED) && !kept.contains(WRAPPER_EXITING),
            "{name}: the wrapper's status lines are not the program's: {receipt}"
        );
        assert!(
            receipt["stdout"]
                .as_str()
                .unwrap_or_default()
                .contains(PROGRAM_LINE),
            "{name}: {receipt}"
        );
    }
}

#[test]
fn a_channel_ended_before_the_program_reported_is_the_channels_failure() {
    let deployment = Deployment::start();
    // The program ends the wrapper that runs it, so the channel ends before
    // the wrapper could report the program's status.
    let program = deployment.place_program("/bin/kill -KILL \"$PPID\"\nexit\n");
    let output = deployment.attach("cut", &program, false);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "an unknown answer is no success: {said}"
    );
    assert!(
        said.contains(CHANNEL_FIRST) && said.contains(NOT_KNOWN),
        "the channel's end is said as the channel's: {said}"
    );
    assert!(
        !said.contains(PROGRAM_EXITED),
        "no program status was reported: {said}"
    );
    assert!(
        !said.contains("refuses"),
        "nothing is said to refuse: {said}"
    );
    assert!(
        said.contains(&format!("blad czekania: {PROGRAM_WAIT}")),
        "the wait on the program failed with the channel's end: {said}"
    );
}
