use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use super::{socket_path, Action, Request, Response, FRAME_LIMIT, SCHEMA};
use crate::release_agent::rollout::serving::owner::process::controller_process_matches;

pub(super) async fn exchange(
    home: Option<&str>,
    action: Action,
) -> Result<Option<Response>, String> {
    let path = socket_path(home)?;
    let inspecting = matches!(&action, Action::Inspect { .. });
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if inspecting && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!(
            "cannot inspect Stado proxy owner socket {}: {error}; listeners require the managed stado serve process",
            path.display()
        )),
    };
    if !metadata.file_type().is_socket() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "Stado proxy control path is not an owner-only socket: {}",
            path.display()
        ));
    }
    let mut stream = match UnixStream::connect(&path).await {
        Ok(stream) => stream,
        Err(error)
            if inspecting
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
        {
            return Ok(None)
        }
        Err(error) => {
            return Err(format!(
                "cannot contact Stado proxy owner at {}: {error}",
                path.display()
            ))
        }
    };
    let peer = stream.peer_cred().map_err(|error| {
        format!(
            "cannot authenticate Stado proxy owner at {}: {error}",
            path.display()
        )
    })?;
    let pid = peer.pid().filter(|pid| *pid > 1).ok_or_else(|| {
        "the operating system did not identify the Stado proxy owner PID".to_string()
    })?;
    if peer.uid() != metadata.uid() || !controller_process_matches(pid)? {
        return Err(format!(
            "control socket {} is owned by pid {pid} uid {}, not the verified stado serve executable",
            path.display(), peer.uid()
        ));
    }
    let request = Request {
        schema_version: SCHEMA,
        action,
    };
    let bytes = serde_json::to_vec(&request)
        .map_err(|error| format!("cannot encode proxy operation: {error}"))?;
    if bytes.len() as u64 > FRAME_LIMIT {
        return Err("release proxy control request exceeds its frame limit".to_string());
    }
    stream
        .write_all(&bytes)
        .await
        .map_err(|error| format!("cannot send proxy operation to pid {pid}: {error}"))?;
    stream
        .shutdown()
        .await
        .map_err(|error| format!("cannot finish proxy request to pid {pid}: {error}"))?;
    let mut bytes = Vec::new();
    stream
        .take(FRAME_LIMIT + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("cannot read proxy response from pid {pid}: {error}"))?;
    if bytes.len() as u64 > FRAME_LIMIT {
        return Err(format!(
            "proxy owner pid {pid} exceeded its response frame limit"
        ));
    }
    let response: Response = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid proxy response from pid {pid}: {error}"))?;
    if response.schema_version != SCHEMA || response.pid != pid {
        return Err(format!(
            "proxy response does not match protocol {SCHEMA} and native peer pid {pid}"
        ));
    }
    if let Some(error) = &response.error {
        return Err(format!(
            "Stado proxy owner pid {pid} refused {:?}: {error}",
            request.action
        ));
    }
    let (state, bind) = match &request.action {
        Action::Ensure { state, bind }
        | Action::Inspect { state, bind }
        | Action::Stop { state, bind } => (state, bind),
    };
    if response
        .proxy
        .as_ref()
        .is_some_and(|proxy| &proxy.state != state || &proxy.bind != bind)
    {
        return Err(format!(
            "proxy owner pid {pid} answered for a different state file or bind"
        ));
    }
    Ok(Some(response))
}
