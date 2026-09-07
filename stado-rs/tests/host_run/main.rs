//! Real current-host journeys for guarded build, attached execution, signal
//! forwarding, and recursive run cleanup. Each fixture declares this machine as
//! a registry target and gives it an isolated HOME, so the production local
//! host-channel branch executes the real compiler and programs without SSH.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

struct Fixture {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
    rustup_home: Option<PathBuf>,
}

impl Fixture {
    fn new() -> Self {
        let original_home = std::env::var_os("HOME").map(PathBuf::from);
        let rustup_home = std::env::var_os("RUSTUP_HOME")
            .map(PathBuf::from)
            .or_else(|| original_home.as_ref().map(|home| home.join(".rustup")));
        let cargo = std::env::var_os("CARGO")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_file())
            .or_else(|| {
                original_home
                    .as_ref()
                    .map(|home| home.join(".cargo/bin/cargo"))
                    .filter(|path| path.is_file())
            })
            .expect("the test is running under an installed Cargo");
        let rustc = cargo
            .parent()
            .map(|directory| directory.join("rustc"))
            .filter(|path| path.is_file())
            .expect("Cargo's installed toolchain directory carries rustc");
        let home = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let hostname = String::from_utf8(Command::new("hostname").output().unwrap().stdout)
            .unwrap()
            .trim()
            .split('.')
            .next()
            .unwrap()
            .chars()
            .filter(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
            .flat_map(char::to_lowercase)
            .collect::<String>();
        let platform = if cfg!(target_arch = "aarch64") {
            "darwin-arm64"
        } else {
            "darwin-amd64"
        };
        std::fs::write(
            storage.path().join("registry.json"),
            format!(
                r#"{{"schema_version":2,"targets":[{{"name":"this-mac","kind":"local","ssh":null,"release_platform":"{platform}","hostnames":["{hostname}"]}}],"coordinators":[]}}"#
            ),
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join(".stado/work/runs")).unwrap();
        let cargo_bin = home.path().join(".cargo/bin");
        std::fs::create_dir_all(&cargo_bin).unwrap();
        std::os::unix::fs::symlink(cargo, cargo_bin.join("cargo")).unwrap();
        std::os::unix::fs::symlink(rustc, cargo_bin.join("rustc")).unwrap();
        Self {
            home,
            storage,
            rustup_home,
        }
    }

    fn run(&self, name: &str) -> PathBuf {
        let path = self.home.path().join(".stado/work/runs").join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(args)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env("STADO_CONFIG", self.storage.path().join("no-config.json"))
            .env("HOME", self.home.path())
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL");
        if let Some(rustup_home) = &self.rustup_home {
            command.env("RUSTUP_HOME", rustup_home);
        }
        command
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON ({error}): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            stderr(output)
        )
    })
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn paths_outside_the_managed_run_area_are_refused_before_host_access() {
    let fixture = Fixture::new();
    let outside = "/tmp/probierz-host-run/Cargo.toml";
    let output = fixture
        .command(&[
            "host",
            "build",
            "this-mac",
            "--manifest-path",
            outside,
            "--bin",
            "probierz",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        stderr(&output).lines().next(),
        Some("Error: path '/tmp/probierz-host-run/Cargo.toml' must be an absolute path below the target account's $HOME/.stado/work/runs, with no '.' or '..' component")
    );

    let output = fixture
        .command(&[
            "host",
            "run-attached",
            "this-mac",
            "--program",
            "/tmp/probierz-host-run/probierz",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        stderr(&output).lines().next(),
        Some("Error: path '/tmp/probierz-host-run/probierz' must be an absolute path below the target account's $HOME/.stado/work/runs, with no '.' or '..' component")
    );

    let output = fixture
        .command(&[
            "host",
            "remove-run-directory",
            "this-mac",
            "/tmp/probierz-host-run",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        stderr(&output).lines().next(),
        Some("Error: path '/tmp/probierz-host-run' must be an absolute path below the target account's $HOME/.stado/work/runs, with no '.' or '..' component")
    );
}

#[test]
fn an_unknown_registry_target_is_named_without_contacting_a_host() {
    let fixture = Fixture::new();
    let manifest = fixture.run("unknown-target").join("Cargo.toml");
    std::fs::write(&manifest, "not contacted").unwrap();
    let output = fixture
        .command(&[
            "host",
            "build",
            "missing-target",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--bin",
            "fixture",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        stderr(&output).lines().next(),
        Some("Error: target 'missing-target' is not in the canonical registry")
    );
}

#[test]
fn a_declared_cargo_build_reports_its_real_output_and_exit_status() {
    let fixture = Fixture::new();
    let run = fixture.run("build-success");
    let source = run.join("source");
    std::fs::create_dir_all(source.join("src")).unwrap();
    std::fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"host-run-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        source.join("Cargo.lock"),
        "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"host-run-fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(source.join("src/main.rs"), "fn main() { println!(\"built\"); }\n").unwrap();

    let output = fixture
        .command(&[
            "host",
            "build",
            "this-mac",
            "--manifest-path",
            source.join("Cargo.toml").to_str().unwrap(),
            "--bin",
            "host-run-fixture",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        stderr(&output)
    );
    let report = json(&output);
    assert_eq!(report["status"], "built");
    assert_eq!(report["exit_code"], 0);
    assert!(report["stderr"].as_str().unwrap().contains("Finished `release`"));
    assert!(source.join("target/release/host-run-fixture").is_file());
}

#[test]
fn attached_execution_consumes_stdin_and_reports_captured_streams() {
    let fixture = Fixture::new();
    let run = fixture.run("attached-stdio");
    let program = run.join("worker");
    executable(
        &program,
        "#!/bin/sh\nIFS= read -r line\nprintf 'worker:%s\\n' \"$line\"\nprintf 'worker diagnostic\\n' >&2\n",
    );
    let mut child = fixture
        .command(&[
            "host",
            "run-attached",
            "this-mac",
            "--program",
            program.to_str().unwrap(),
            "--arg",
            "stado",
            "--arg",
            "byk-auth-worker",
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"bridgeToken\":\"secret-value\"}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let report = json(&output);
    assert_eq!(report["status"], "exited");
    assert_eq!(report["exit_code"], 0);
    assert_eq!(
        report["stdout"],
        "worker:{\"bridgeToken\":\"secret-value\"}\n"
    );
    assert_eq!(report["stderr"], "worker diagnostic\n");
    let arguments = report["arguments"].as_array().unwrap();
    assert_eq!(arguments, &[Value::from("stado"), Value::from("byk-auth-worker")]);
}

#[test]
fn sigterm_is_forwarded_to_the_attached_program() {
    let fixture = Fixture::new();
    let run = fixture.run("attached-signal");
    let ready = run.join("ready");
    let program = run.join("worker");
    executable(
        &program,
        &format!(
            "#!/bin/sh\ntrap \"printf 'forwarded-term\\\\n'; exit 42\" TERM\nprintf ready > {}\nwhile :; do :; done\n",
            ready.display()
        ),
    );
    let child = fixture
        .command(&[
            "host",
            "run-attached",
            "this-mac",
            "--program",
            program.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let signal_root = fixture.home.path().join(".stado/work/run-signals");
    while (!ready.exists()
        || std::fs::read_dir(&signal_root)
            .map(|entries| entries.count() == 0)
            .unwrap_or(true))
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists(), "attached worker never started");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(child.id() as i32),
        nix::sys::signal::Signal::SIGTERM,
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(42), "{}", stderr(&output));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "forwarded-term\n");
}

#[test]
fn recursive_run_cleanup_is_idempotent_and_cannot_name_a_nested_subtree() {
    let fixture = Fixture::new();
    let run = fixture.run("cleanup");
    std::fs::create_dir_all(run.join("source/nested")).unwrap();
    std::fs::write(run.join("source/nested/artifact"), b"bytes").unwrap();

    let output = fixture
        .command(&[
            "host",
            "remove-run-directory",
            "this-mac",
            run.join("source").to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(run.exists());

    let output = fixture
        .command(&[
            "host",
            "remove-run-directory",
            "this-mac",
            run.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(json(&output)["status"], "removed");
    assert!(!run.exists());

    let output = fixture
        .command(&[
            "host",
            "remove-run-directory",
            "this-mac",
            run.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(json(&output)["status"], "absent");
}
