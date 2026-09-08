//! The one remote read this command family owns: a fixed base64-framed program
//! on the registry ssh channel, and the typed readers built on top of it.

use crate::cli::CmdError;
use crate::deploy::{host_channel, production_runner, shlex_quote};
use crate::targets::ComputeTarget;

use super::splice;

mod script;
mod state;

use script::READ_TEMPLATE;

pub(crate) use state::{remote_host_state, remote_read, remote_read_head, remote_read_tail};

/// What the host said about one file: the full size it reported, and the bytes
/// it sent.
struct RemoteFile {
    bytes: u64,
    content: Vec<u8>,
}

async fn read_remote(
    host: &ComputeTarget,
    path: &str,
    body: &str,
) -> Result<Option<RemoteFile>, CmdError> {
    let script = splice(
        READ_TEMPLATE,
        &[("@PATH@", &shlex_quote(path)), ("@BODY@", body)],
    );
    let runner = production_runner();
    let output = host_channel::run_script(host, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: cannot read {path}: {}",
            host.name,
            host_channel::last_error_line(&output, "remote read failed")
        )));
    }
    let mut bytes: Option<u64> = None;
    let mut encoded: Option<&str> = None;
    for line in output.stdout.lines() {
        let fields = host_channel::marker_fields(line);
        match fields.first().copied() {
            Some("STADO_QUARANTINE_ABSENT") => return Ok(None),
            Some("STADO_QUARANTINE_BYTES") => {
                bytes = fields.get(1).and_then(|value| value.parse().ok());
            }
            Some("STADO_QUARANTINE_BASE64") => encoded = Some(fields.get(1).copied().unwrap_or("")),
            _ => {}
        }
    }
    let (Some(bytes), Some(encoded)) = (bytes, encoded) else {
        return Err(CmdError::click(format!(
            "{}: answered nothing usable about {path}",
            host.name
        )));
    };
    let content = if encoded.is_empty() {
        Vec::new()
    } else {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        BASE64.decode(encoded).map_err(|error| {
            CmdError::click(format!(
                "{}: {path} came back unreadable: {error}",
                host.name
            ))
        })?
    };
    Ok(Some(RemoteFile { bytes, content }))
}
