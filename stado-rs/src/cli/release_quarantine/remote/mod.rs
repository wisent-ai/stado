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
        .map_err(CmdError::from)?;
    if !output.succeeded() {
        return Err(CmdError::click(format!(
            "{}: cannot read {path}: {}",
            host.name,
            host_channel::last_error_line(&output, "remote read failed")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let mut bytes: Option<u64> = None;
    let mut encoded: Option<&str> = None;
    for line in output.stdout.lines() {
        let fields = host_channel::marker_fields(line);
        let unreadable = |detail: &str| {
            CmdError::click(format!("{}: {path}: {detail}", host.name))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        };
        match fields.as_slice() {
            ["STADO_QUARANTINE_ABSENT", ..] => return Ok(None),
            ["STADO_QUARANTINE_BYTES", field, ..] => {
                bytes = host_channel::marker_number(field, "byte count")
                    .map_err(|detail| unreadable(&detail))?;
            }
            ["STADO_QUARANTINE_BYTES"] => return Err(unreadable("its byte count line has no value")),
            // An empty file prints an empty field; a line without the field
            // at all is not an answer about the content.
            ["STADO_QUARANTINE_BASE64", field, ..] => encoded = Some(*field),
            ["STADO_QUARANTINE_BASE64"] => return Err(unreadable("its content line has no value")),
            _ => {}
        }
    }
    let (Some(bytes), Some(encoded)) = (bytes, encoded) else {
        return Err(CmdError::click(format!(
            "{}: answered nothing usable about {path}",
            host.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
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
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?
    };
    Ok(Some(RemoteFile { bytes, content }))
}
