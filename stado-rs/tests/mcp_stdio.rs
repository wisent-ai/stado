//! End-to-end `stado-mcp` smoke test: newline-delimited JSON-RPC through
//! the real binary's stdin/stdout, with STADO_BIN pointing at a fake
//! `stado` script for tools/call dispatch. (The in-process transport is
//! covered exhaustively by the unit tests in src/mcp.rs.)

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn stdio_session_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("fake-stado");
    std::fs::write(&fake, "#!/bin/sh\nprintf 'canned-output\\n'\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_stado-mcp"))
        .env("STADO_BIN", &fake)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("stado-mcp binary runs");
    let input = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "garbage\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"stado_vast_status\"}}\n",
    );
    child.stdin.as_mut().unwrap().write_all(input.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());

    let lines: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 3, "notification produced no frame: {lines:?}");
    assert_eq!(lines[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "stado");
    assert_eq!(lines[1]["id"], serde_json::Value::Null);
    assert_eq!(lines[1]["error"]["code"], -32700);
    assert_eq!(
        lines[2]["result"],
        serde_json::json!({"content": [{"type": "text", "text": "canned-output\n"}]})
    );
}
