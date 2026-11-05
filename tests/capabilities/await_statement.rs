//! `stado capabilities call --await-statement`, run as the real binary in an
//! isolated home where Brama is placed nowhere, so the attestation
//! alternative serves: a call for a request no statement answers yet keeps
//! running instead of refusing, and prints, as its own answer, the statement
//! `stado capabilities attestation record` writes while it waits, labelled as
//! an attestation. Without the flag the same kind of request is refused, as
//! before, naming the flag.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const CONSUMER: &str = "oko";

fn stado(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("PATH names the tools"))
        .env("HOME", root)
        .env("STADO_CONFIG", root.join(".stado").join("config.json"))
        .env("TMPDIR", root.join("tmp"))
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", root.join("store"))
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run(root: &Path, args: &[&str], input: &str) -> Output {
    let mut child = stado(root, args).stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn call(root: &Path, request: &Value, wait: bool) -> Child {
    let mut args = vec!["capabilities", "call", "inference", "--consumer", CONSUMER, "--json"];
    if wait {
        args.push("--await-statement");
    }
    let mut child = stado(root, &args).stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(request.to_string().as_bytes()).unwrap();
    child
}

/// The digests of the requests kept for the consumer so far.
fn kept(root: &Path) -> Vec<String> {
    let directory = root.join(".stado/attestations/inference").join(CONSUMER);
    std::fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".request.json").map(str::to_owned))
        .collect()
}

/// The digest of the request a call keeps, once it keeps it: the one kept
/// request that is not among `before`.
fn newly_kept(root: &Path, before: &[String], started: Instant) -> String {
    loop {
        if let Some(digest) = kept(root).into_iter().find(|digest| !before.contains(digest)) {
            return digest;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "the call kept no new request under {}", root.display());
        std::thread::yield_now();
    }
}

#[test]
fn a_call_that_awaits_a_statement_is_answered_by_the_one_recorded_while_it_waits() {
    let root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the package sits inside the checkout")
        .join(".build/capabilities-await-statement")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    std::fs::create_dir_all(root.join("store")).unwrap();
    let initialised = run(&root, &["config", "init"], "");
    assert!(initialised.status.success(), "{}", String::from_utf8_lossy(&initialised.stderr));

    let question = |marker: &str| {
        json!({
            "model": "any-vision-capable",
            "messages": [{"role": "user", "content": format!("Does the page show {marker}?")}],
            "tools": [{"type": "function", "function": {"name": "answer", "parameters": {"type": "object"}}}],
            "tool_choice": {"type": "function", "function": {"name": "answer"}}
        })
    };

    // Without the flag a request no statement answers is refused, naming it.
    let refused = call(&root, &question("a refusal"), false).wait_with_output().unwrap();
    assert!(!refused.status.success(), "a request nothing answers was answered");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("passes --await-statement is answered when it is recorded"), "{said}");

    // With the flag the call waits, and the statement recorded meanwhile is
    // its answer.
    let before = kept(&root);
    let waiting = call(&root, &question("the sign-in choices"), true);
    let digest = newly_kept(&root, &before, Instant::now());
    let recorded = run(
        &root,
        &["capabilities", "attestation", "record", "inference", "--consumer", CONSUMER, &digest, "--checked", "journey"],
        r#"{"shows": true}"#,
    );
    assert!(recorded.status.success(), "{}", String::from_utf8_lossy(&recorded.stderr));
    let answered = waiting.wait_with_output().unwrap();
    assert!(answered.status.success(), "{}", String::from_utf8_lossy(&answered.stderr));
    let printed: Value = serde_json::from_slice(&answered.stdout).unwrap();
    assert_eq!(printed["resolution"]["provider"], "attestation");
    assert_eq!(printed["answer"]["model"], "attestation");
    assert_eq!(printed["answer"]["attestation"]["request"], digest);
    let arguments = printed["answer"]["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
        .as_str()
        .unwrap();
    assert_eq!(serde_json::from_str::<Value>(arguments).unwrap(), json!({"shows": true}));
    std::fs::remove_dir_all(&root).unwrap();
}
