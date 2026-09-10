//! Attach the real Jeden RPC process, create its real ledger, then reconnect.
//! No launcher or provider stand-in is installed in the isolated home.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::harness::{said, Area, TARGET};

const RPC_DEADLINE: Duration = Duration::from_secs(60);
const PROCESS_POLL: Duration = Duration::from_millis(50);

fn install_runtime(area: &Area) {
    let binary = std::env::var_os("JEDEN_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("the runner has a home"))
                .join(".stado/bin/jeden")
        });
    let binary = fs::canonicalize(&binary).unwrap_or_else(|error| {
        panic!(
            "blocked: the real installed Jeden binary {} cannot be read: {error}",
            binary.display()
        )
    });
    let identity = Command::new(&binary)
        .arg("--version")
        .output()
        .expect("the real Jeden binary can start");
    assert!(
        identity.status.success(),
        "Jeden identity: {}",
        said(&identity.stderr)
    );
    let destination = area.home.join(".stado/bin");
    fs::create_dir_all(&destination).unwrap();
    std::os::unix::fs::symlink(&binary, destination.join("jeden")).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_stado"), destination.join("stado")).unwrap();
    fs::write(area.root.join("binaries.json"), serde_json::to_vec_pretty(&json!({
        "jeden": {
            "path": binary,
            "version": said(&identity.stdout).trim(),
            "sha256": hex::encode(Sha256::digest(fs::read(&binary).unwrap())),
        },
        "stado": {
            "path": env!("CARGO_BIN_EXE_stado"),
            "sha256": hex::encode(Sha256::digest(fs::read(env!("CARGO_BIN_EXE_stado")).unwrap())),
        },
    })).unwrap()).unwrap();
}

fn attach(area: &Area, name: &str, resume: Option<&str>, requests: &[Value]) -> Output {
    let directory = area.root.join("attachments").join(name);
    fs::create_dir_all(&directory).unwrap();
    let mut args = vec![
        "workload",
        "attach",
        "jeden-session",
        "--target",
        TARGET,
        "--workspace",
        "__home__",
    ];
    if let Some(session) = resume {
        args.extend(["--resume", session]);
    }
    let mut frames = Vec::new();
    for request in requests {
        serde_json::to_writer(&mut frames, request).unwrap();
        frames.push(b'\n');
    }
    fs::write(
        directory.join("request.json"),
        serde_json::to_vec_pretty(&json!({
            "args": args, "frames": requests,
        }))
        .unwrap(),
    )
    .unwrap();
    let stdout = directory.join("stdout");
    let stderr = directory.join("stderr");
    let mut child = area
        .command(&args)
        .env("JEDEN_SESSION_ROOT", area.home.join(".jeden/sessions"))
        .env_remove("JEDEN_CONFIG")
        .env_remove("JEDEN_CONFIG_PATH")
        .stdin(Stdio::piped())
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap())
        .spawn()
        .expect("the real attachment process starts");
    child.stdin.take().unwrap().write_all(&frames).unwrap();
    let deadline = Instant::now() + RPC_DEADLINE;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "attachment timed out; retained output: {}",
                directory.display()
            );
        }
        std::thread::sleep(PROCESS_POLL);
    };
    fs::write(
        directory.join("result.json"),
        serde_json::to_vec(&json!({"exit_code": status.code()})).unwrap(),
    )
    .unwrap();
    Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}

fn reply(output: &Output, id: &str) -> Value {
    assert!(
        output.status.success(),
        "attachment failed: {}",
        said(&output.stderr)
    );
    let response = said(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value["id"] == id)
        .unwrap_or_else(|| panic!("no RPC reply {id}: {}", said(&output.stdout)));
    assert!(response.get("error").is_none(), "RPC failed: {response}");
    response["result"].clone()
}

#[test]
fn native_attachment_creates_a_real_ledger_and_refuses_a_missing_resume_ledger() {
    let area = Area::new();
    install_runtime(&area);
    let home = fs::canonicalize(&area.home).unwrap();
    let opened = attach(
        &area,
        "create",
        None,
        &[
            json!({"id": "create", "method": "session/new", "params": {"cwd": home}}),
            json!({"id": "shutdown", "method": "shutdown"}),
        ],
    );
    let created = reply(&opened, "create");
    let ledger = Path::new(
        created["sessionPath"]
            .as_str()
            .expect("Jeden names its real ledger"),
    );
    let ledger = fs::canonicalize(ledger).unwrap();
    assert!(
        ledger.starts_with(home.join(".jeden/sessions")),
        "Jeden wrote outside the isolated home"
    );
    let state = fs::read(ledger.join("state.json")).expect("Jeden persisted session state");
    let document: Value = serde_json::from_slice(&state).unwrap();
    assert_eq!(document["cwd"], home.to_string_lossy().as_ref());
    let transcript =
        fs::read(ledger.join("transcript.jsonl")).expect("Jeden persisted its event ledger");
    let session = ledger.file_name().unwrap().to_str().unwrap();
    let reconnected = attach(
        &area,
        "reconnect",
        Some(session),
        &[
            json!({"id": "initialize", "method": "initialize"}),
            json!({"id": "shutdown", "method": "shutdown"}),
        ],
    );
    let initialized = reply(&reconnected, "initialize");
    assert_eq!(initialized["protocol"], "jeden-rpc");
    assert_eq!(fs::read(ledger.join("state.json")).unwrap(), state);
    assert_eq!(
        fs::read(ledger.join("transcript.jsonl")).unwrap(),
        transcript
    );

    let missing = area.stado(&[
        "workload",
        "attach",
        "jeden-session",
        "--target",
        TARGET,
        "--workspace",
        "__home__",
        "--resume",
        "absent-session",
    ]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        said(&missing.stderr).contains("session ledger is missing:"),
        "{}",
        said(&missing.stderr)
    );
    assert!(!area.home.join(".jeden/sessions/absent-session").exists());
    assert_eq!(fs::read(ledger.join("state.json")).unwrap(), state);
}
