//! The published object: what an entrance is once it exists, how it is
//! rendered into the store, how it is read back, and the one read every other
//! subcommand starts from.

use serde_json::{json, Value};

use crate::queue::JobStorage;

use super::INGRESS_PATH;

/// Enough about the two processes for `down` and `status` to act without
/// guessing: which machine they belong to, which process *group* to signal, and
/// where each one's output went.
///
/// `machine` is not decoration. The object lives in a store the whole fleet
/// reads, and a pid from another host is a pid on this one too — signalling it
/// would kill something unrelated with no way to tell afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PidHint {
    pub machine: String,
    pub listener_pgid: i32,
    pub tunnel_pgid: i32,
    pub listener_log: String,
    pub tunnel_log: String,
}

/// The entrance, as published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingress {
    pub base_url: String,
    pub mode: String,
    pub host: String,
    pub started_at: String,
    pub verified_at: String,
    pub listener_port: u16,
    pub pid_hint: PidHint,
}

/// Render the entrance as its stored document. Pure.
pub fn ingress_document(ingress: &Ingress) -> Value {
    json!({
        "base_url": ingress.base_url,
        "mode": ingress.mode,
        "host": ingress.host,
        "started_at": ingress.started_at,
        "verified_at": ingress.verified_at,
        "listener_port": ingress.listener_port,
        "pid_hint": {
            "machine": ingress.pid_hint.machine,
            "listener_pgid": ingress.pid_hint.listener_pgid,
            "tunnel_pgid": ingress.pid_hint.tunnel_pgid,
            "listener_log": ingress.pid_hint.listener_log,
            "tunnel_log": ingress.pid_hint.tunnel_log,
        },
    })
}

/// Parse a stored entrance. Pure.
///
/// Strict about `base_url` and the two group ids, because those are what the
/// other two subcommands act on: an object missing either is not something to
/// half-read, it is something `down` cannot honour.
pub fn parse_ingress(document: &Value) -> Result<Ingress, String> {
    let text = |key: &str| -> Result<String, String> {
        document
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("ingress object has no string '{key}'"))
    };
    let hint = document
        .get("pid_hint")
        .ok_or_else(|| "ingress object has no 'pid_hint'".to_string())?;
    // A process group at or below init's pid is never one `up` started:
    // signalling group 1, or every group, would hit the whole machine.
    // <https://man7.org/linux/man-pages/man2/kill.2.html>
    let pgid = |key: &str| -> Result<i32, String> {
        let stated = hint
            .get(key)
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("ingress pid_hint has no integer '{key}'"))?;
        let group = i32::try_from(stated)
            .map_err(|error| format!("ingress pid_hint '{key}' {stated} is not a process group id: {error}"))?;
        if group <= INIT_PID {
            return Err(format!(
                "ingress pid_hint '{key}' {group} names init or every process, never a group `up` started"
            ));
        }
        Ok(group)
    };
    // The machine is what keeps `down` from signalling another host's pids
    // here, and the logs are where `status` points: an object without them
    // is refused like one without its pgids.
    let hint_text = |key: &str| -> Result<String, String> {
        hint.get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("ingress pid_hint has no '{key}'"))
    };
    let listener_port = document
        .get("listener_port")
        .and_then(Value::as_u64)
        .ok_or_else(|| "ingress object has no integer 'listener_port'".to_string())?;
    Ok(Ingress {
        base_url: text("base_url")?,
        mode: text("mode")?,
        host: text("host")?,
        started_at: text("started_at")?,
        verified_at: text("verified_at")?,
        listener_port: u16::try_from(listener_port).map_err(|error| {
            format!("ingress listener_port {listener_port} is not a port: {error}")
        })?,
        pid_hint: PidHint {
            machine: hint_text("machine")?,
            listener_pgid: pgid("listener_pgid")?,
            tunnel_pgid: pgid("tunnel_pgid")?,
            listener_log: hint_text("listener_log")?,
            tunnel_log: hint_text("tunnel_log")?,
        },
    })
}

/// The pid of init. <https://man7.org/linux/man-pages/man2/kill.2.html>
const INIT_PID: i32 = 1;

/// The entrance this store currently publishes, if any. An object that cannot
/// be parsed is reported as a parse error rather than as "nothing published":
/// silently treating a corrupt object as absent is how a live tunnel becomes
/// unreachable by `down`.
pub async fn published(store: &JobStorage) -> Result<Option<Ingress>, crate::cli::CmdError> {
    use crate::cli::CmdError;
    use crate::primitives::failure::FailureCode;
    let Some(text) = store.download_text(INGRESS_PATH).await? else {
        return Ok(None);
    };
    let document: Value = serde_json::from_str(&text).map_err(|exc| {
        CmdError::click(format!(
            "the published ingress object at {INGRESS_PATH} is not JSON ({exc})"
        ))
        .stating(FailureCode::InfraDown)
    })?;
    parse_ingress(&document).map(Some).map_err(|exc| {
        CmdError::click(format!(
            "the published ingress object at {INGRESS_PATH}: {exc}"
        ))
        .stating(FailureCode::InfraDown)
    })
}
