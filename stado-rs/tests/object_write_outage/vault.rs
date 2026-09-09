//! The authorization the object routes require, answered on loopback.
//!
//! `authorize_object` reads the namespace's bearer out of Skarbiec through the
//! object verifier, so a request to the write plane cannot be authorized
//! without a broker. This is a stand-in broker on a loopback port: it lists
//! the verifier items the configured namespaces name, and answers each item's
//! read with that item's bearer. It holds no real credential and cannot reach
//! a real vault.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::Value;

/// The Skarbiec item holding one namespace's bearer, named the way
/// `config::parse_object_api_namespaces` requires.
pub fn verifier_item(namespace: &str) -> String {
    if namespace == "wisent-backend" {
        "wisent-backend-object-client".to_string()
    } else {
        format!("{namespace}-object-api")
    }
}

/// The bearer this broker holds for one namespace, and therefore the one the
/// route expects on the wire.
pub fn namespace_token(namespace: &str) -> String {
    format!("{}-token", verifier_item(namespace))
}

/// The `WC_OBJECT_API_NAMESPACES` document: every active namespace with its
/// own item and its `data/` subtree. A missing active namespace is a
/// configuration problem the verifier reports instead of reaching the vault.
///
/// The queue's own namespace additionally grants every canonical queue
/// prefix. That is not decoration: startup validation refuses a policy which
/// leaves the queue's prefixes ungranted — an object API whose own queue
/// cannot be read answers 401 to every agent claim — and the refusal shuts the
/// object boundary before any route runs.
pub fn namespaces_document() -> String {
    let entries = stado::config::ACTIVE_OBJECT_NAMESPACES
        .iter()
        .map(|namespace| {
            let mut prefixes = vec!["data/"];
            if *namespace == stado::config::QUEUE_OBJECT_NAMESPACE {
                prefixes.extend_from_slice(stado::queue::copy::CANONICAL_PREFIXES);
            }
            let prefixes = prefixes
                .iter()
                .map(|prefix| format!("\"{prefix}\""))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                r#""{namespace}": {{"item": "{}", "prefixes": [{prefixes}]}}"#,
                verifier_item(namespace)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{entries}}}")
}

/// An owner-only grant file. `skarbiec::read_grant` refuses anything a group
/// or another user can read, so the mode is part of the fixture.
pub fn write_grant(path: &Path, token: &str) {
    std::fs::write(path, token).expect("grant file is writable");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .expect("grant file takes owner-only mode");
}

/// Read one HTTP message off `stream`: the head, then `Content-Length` bytes.
pub fn read_message(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut raw = Vec::new();
    let mut byte = [0_u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => raw.push(byte[0]),
        }
    }
    let head = String::from_utf8_lossy(&raw).into_owned();
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or_default();
    let mut body = vec![0_u8; length];
    if length > 0 && stream.read_exact(&mut body).is_err() {
        return None;
    }
    Some((head, String::from_utf8_lossy(&body).into_owned()))
}

fn write_response(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Spawn the broker and return the origin the verifier should be pointed at.
pub fn spawn() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("stand-in broker binds loopback");
    let addr = listener
        .local_addr()
        .expect("stand-in broker has an address");
    std::thread::Builder::new()
        .name("outage-stand-in-skarbiec".to_string())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || answer(&mut stream));
            }
        })
        .expect("stand-in broker thread starts");
    addr
}

fn answer(stream: &mut TcpStream) {
    let Some((head, body)) = read_message(stream) else {
        return;
    };
    let target = head.split_whitespace().nth(usize::from(true)).unwrap_or("");
    match target {
        "/v1/items/list" => {
            let items = stado::config::ACTIVE_OBJECT_NAMESPACES
                .iter()
                .map(|namespace| {
                    format!(
                        r#"{{"id": "{}", "deleted": false}}"#,
                        verifier_item(namespace)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            write_response(stream, 200, "OK", &format!("[{items}]"));
        }
        "/v1/items/read" => {
            let request: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let id = request
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            write_response(stream, 200, "OK", &format!(r#"{{"value": "{id}-token"}}"#));
        }
        _ => write_response(stream, 404, "Not Found", "{}"),
    }
}
