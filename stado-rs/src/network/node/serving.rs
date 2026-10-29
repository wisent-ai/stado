//! What the node says about itself once it listens or fails to: one file
//! beside its key, written by the node and awaited by `stado network up`
//! through the kernel's own directory notification, so `up` neither polls
//! nor guesses how long a node takes to start.

use serde::{Deserialize, Serialize};

use crate::network::key;

/// The node listens.
pub const SERVING: &str = "serving";
/// The node stopped before it could listen.
pub const FAILED: &str = "failed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Serving {
    pub pid: u32,
    pub port: u16,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn path() -> Result<std::path::PathBuf, String> {
    Ok(key::directory()?.join("serving.json"))
}

/// What the node last published, `None` when nothing is published.
pub fn read() -> Result<Option<Serving>, String> {
    let path = path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("{} is not a node publication: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

pub(super) fn publish(serving: &Serving) -> Result<(), String> {
    let path = path()?;
    let text = serde_json::to_string(serving).map_err(|error| format!("encode: {error}"))?;
    crate::targets::write_atomic(&path, &text)
        .map_err(|error| format!("write {}: {error}", path.display()))
}

/// Wait until a node listening on `port` publishes that it serves, or a node
/// other than `before` publishes that it failed. A publication from a node
/// that is still running on that port is the answer at once: loading a unit
/// that is already running starts nothing new.
pub fn await_serving(port: u16, before: Option<&Serving>) -> Result<Serving, String> {
    let path = path()?;
    let directory = key::directory()?;
    let answered = |serving: &Serving| -> Option<Result<Serving, String>> {
        let alive = crate::providers::local::helpers::pid_alive(serving.pid as i32);
        if serving.state == SERVING && serving.port == port && alive {
            return Some(Ok(serving.clone()));
        }
        let new = before.is_none_or(|before| before.pid != serving.pid);
        if serving.state == FAILED && new {
            return Some(Err(format!(
                "the network node (pid {}) stopped: {}",
                serving.pid,
                serving
                    .reason
                    .as_deref()
                    .map_or("it published no reason", |reason| reason)
            )));
        }
        None
    };
    crate::wait::blocking(
        crate::wait::Kind::Process,
        format!("the Stado network node to publish that it listens on port {port}"),
        path.display(),
        || loop {
            // Armed before the read, so a publication between the two still
            // wakes the wait.
            let mut watch =
                crate::queue::local_file::watch_directories(std::slice::from_ref(&directory))
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
            if let Some(answer) = read()
                .map_err(std::io::Error::other)?
                .as_ref()
                .and_then(answered)
            {
                return answer.map_err(std::io::Error::other);
            }
            watch
                .next()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
        },
    )
    .map_err(|error| error.to_string())
}
