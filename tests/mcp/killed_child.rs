//! A tool call whose `stado` child is ended by a signal is answered with
//! that cause, not with an exit code no process returned.
//!
//! The shared subprocess capture once reported a child a signal ended as an
//! invented exit code, and the MCP server named every spawn failure "stado
//! CLI not found". The real `stado mcp` runs with an isolated `HOME` under
//! Cargo's test directory, and its tools' `stado` children read a fleet store
//! this test owns, which accepts every connection and answers none:
//!
//! - `stado_status` starts `stado status`, which stands on the silent store;
//!   once the store holds its connection, the child is ended with SIGKILL,
//!   and the answer is an error saying `stado status did not run to an exit`
//!   because it was ended by that signal and returned no exit code;
//! - with `STADO_BIN` naming a program that does not exist, the answer says
//!   the child did not run to an exit, not that the CLI is missing.
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;

use serde_json::{json, Value};

struct Run {
    root: PathBuf,
    report: Value,
}

impl Run {
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("mcp-killed-child")
            .join(format!("{name}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        let revision = Command::new("git")
            .arg("-C")
            .arg(env!("CARGO_MANIFEST_DIR"))
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git answers the checkout's revision");
        Self {
            report: json!({
                "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
                "exchanges": [],
                "outcome": "failed",
            }),
            root,
        }
    }

    /// The real stado with nothing of this account's configuration.
    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join("no-config.json"))
            .env("TMPDIR", self.root.join("tmp"));
        command
    }

    /// A fleet store that accepts every connection, keeps it and answers
    /// none, on a loopback port Stado hands out; each accepted connection
    /// arrives on the receiver.
    fn silent_store(&self) -> (String, mpsc::Receiver<TcpStream>) {
        let printed = self
            .stado()
            .args(["host", "local", "port"])
            .output()
            .expect("stado host local port runs");
        assert!(printed.status.success(), "{printed:?}");
        let printed = String::from_utf8_lossy(&printed.stdout);
        let Ok(port) = printed.trim().parse::<u16>() else {
            panic!("stado host local port printed {printed:?}");
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (keep, held) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if keep.send(stream).is_err() {
                    return;
                }
            }
        });
        (address, held)
    }

    /// The real `stado mcp`, with every tool's child reading `store`.
    fn mcp(&self, store: &str, stado_bin: Option<&Path>) -> (Child, BufReader<ChildStdout>) {
        let token = self.root.join("object-api.token");
        std::fs::write(&token, "mcp-killed-child").unwrap();
        let mut command = self.stado();
        command
            .arg("mcp")
            .env("WC_STORAGE_BACKEND", "stado")
            .env("WC_STADO_STORAGE_URL", format!("http://{store}"))
            .env("WC_STADO_STORAGE_TOKEN_FILE", &token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(self.root.join("mcp.stderr.log")).unwrap());
        if let Some(bin) = stado_bin {
            command.env("STADO_BIN", bin);
        }
        let mut child = command.spawn().expect("start the real stado mcp");
        let stdout = BufReader::new(child.stdout.take().unwrap());
        (child, stdout)
    }

    fn send(&mut self, child: &mut Child, request: &Value) {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, "{request}").unwrap();
        stdin.flush().unwrap();
        self.report["exchanges"]
            .as_array_mut()
            .unwrap()
            .push(json!({"request": request}));
        self.save();
    }

    fn answer(&mut self, stdout: &mut BufReader<ChildStdout>) -> Value {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        self.report["exchanges"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap()["response"] = json!(line);
        self.save();
        match serde_json::from_str(&line) {
            Ok(answer) => answer,
            Err(error) => panic!("stado mcp answered {line:?}, which is not JSON: {error}"),
        }
    }

    fn save(&self) {
        std::fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

fn call(id: &str, tool: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": tool, "arguments": {}},
    })
}

fn refusal(answer: &Value) -> String {
    let Some(message) = answer["error"]["message"].as_str() else {
        panic!("the tool call was not refused: {answer}");
    };
    message.to_owned()
}

/// The children `parent` started, as the kernel lists them.
fn children(parent: u32) -> Vec<String> {
    let listed = Command::new("/usr/bin/pgrep")
        .arg("-P")
        .arg(parent.to_string())
        .output()
        .expect("pgrep lists a process's children");
    String::from_utf8_lossy(&listed.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_child_ended_by_a_signal_is_answered_with_that_signal() {
    let mut run = Run::new("signal");
    let (store, held) = run.silent_store();
    let (mut mcp, mut stdout) = run.mcp(&store, None);
    run.send(&mut mcp, &call("status-killed", "stado_status"));

    // The child stands on the silent store once the store holds its read.
    let connection = held
        .recv()
        .expect("stado status opens a connection to the store");
    let listed = children(mcp.id());
    let [child] = listed.as_slice() else {
        panic!("stado mcp runs one child for the call, not {listed:?}");
    };
    let killed = Command::new("/bin/kill")
        .args(["-s", "KILL", child])
        .status()
        .expect("kill sends the signal");
    assert!(killed.success(), "kill -s KILL {child}: {killed}");
    run.report["killed_child"] = json!(child);

    let answer = run.answer(&mut stdout);
    drop(connection);
    let message = refusal(&answer);
    assert!(
        message.contains("stado status did not run to an exit: "),
        "the refusal does not say the child did not run to an exit: {message}"
    );
    assert!(
        message.contains(&format!(
            "was ended by signal {}",
            nix::sys::signal::Signal::SIGKILL as i32
        )) && message.contains("returned no exit code"),
        "the refusal does not name the signal that ended the child: {message}"
    );
    assert!(
        !message.contains("exited nonzero"),
        "the refusal reports an exit the child never made: {message}"
    );
    mcp.kill().unwrap();
    mcp.wait().unwrap();
    run.report["outcome"] = json!("passed");
    run.save();
}

#[test]
fn a_child_that_cannot_start_is_answered_with_the_spawn_error() {
    let mut run = Run::new("spawn");
    let (store, _held) = run.silent_store();
    let absent = run.root.join("absent-stado");
    let (mut mcp, mut stdout) = run.mcp(&store, Some(&absent));
    run.send(&mut mcp, &call("status-unstarted", "stado_status"));
    let message = refusal(&run.answer(&mut stdout));
    assert!(
        message.contains("stado status did not run to an exit: "),
        "the refusal does not say the child did not run to an exit: {message}"
    );
    assert!(
        !message.contains("stado CLI not found"),
        "a spawn failure is still named as a missing CLI: {message}"
    );
    mcp.kill().unwrap();
    mcp.wait().unwrap();
    run.report["outcome"] = json!("passed");
    run.save();
}
