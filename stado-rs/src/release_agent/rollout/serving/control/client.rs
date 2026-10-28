use nix::libc;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use super::{socket_path, Action, ControlClientError, Request, Response, PROTOCOL};
use crate::release_agent::rollout::serving::owner::process::controller_process_matches;

// The kernel names PID 1 for the init process
// (https://man7.org/linux/man-pages/man1/init.1.html,
// https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/Lifecycle.html),
// which is never the stado serve process this socket must belong to.
const INIT_PID: i32 = 1;

pub(super) async fn exchange(
    home: Option<&str>,
    action: Action,
) -> Result<Option<Response>, ControlClientError> {
    let path = socket_path(home).map_err(ControlClientError::Config)?;
    let inspecting = action.is_inspection();
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if inspecting && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ControlClientError::Io {
                context: format!(
                    "cannot inspect Stado proxy owner socket {}; listeners require the managed stado serve process",
                    path.display()
                ),
                error,
            })
        }
    };
    let group_or_world = crate::primitives::file_mode::widen(libc::S_IRWXG | libc::S_IRWXO);
    if !metadata.file_type().is_socket()
        || std::num::NonZeroU32::new(metadata.permissions().mode() & group_or_world).is_some()
    {
        return Err(ControlClientError::Refused(format!(
            "Stado proxy control path is not an owner-only socket: {}",
            path.display()
        )));
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
            return Err(ControlClientError::Io {
                context: format!("cannot contact Stado proxy owner at {}", path.display()),
                error,
            })
        }
    };
    let peer = stream.peer_cred().map_err(ControlClientError::io(format!(
        "cannot authenticate Stado proxy owner at {}",
        path.display()
    )))?;
    let pid = peer.pid().filter(|pid| *pid > INIT_PID).ok_or_else(|| {
        ControlClientError::Refused(
            "the operating system did not identify the Stado proxy owner PID".to_string(),
        )
    })?;
    let matches = controller_process_matches(pid).map_err(ControlClientError::Unavailable)?;
    if peer.uid() != metadata.uid() || !matches {
        return Err(ControlClientError::Refused(format!(
            "control socket {} is owned by pid {pid} uid {}, not the verified stado serve executable",
            path.display(), peer.uid()
        )));
    }
    let request = Request {
        protocol: PROTOCOL.to_string(),
        action,
    };
    let bytes = serde_json::to_vec(&request).map_err(|error| {
        ControlClientError::Refused(format!("cannot encode proxy operation: {error}"))
    })?;
    stream
        .write_all(&bytes)
        .await
        .map_err(ControlClientError::io(format!(
            "cannot send proxy operation to pid {pid}"
        )))?;
    stream
        .shutdown()
        .await
        .map_err(ControlClientError::io(format!(
            "cannot finish proxy request to pid {pid}"
        )))?;
    // Both ends are the owner's own processes (peer credentials checked
    // above), so the answer is read whole, to the end the owner writes.
    let mut bytes = Vec::new();
    stream
        .read_to_end(&mut bytes)
        .await
        .map_err(ControlClientError::io(format!(
            "cannot read proxy response from pid {pid}"
        )))?;
    let response: Response = serde_json::from_slice(&bytes).map_err(|error| {
        ControlClientError::Refused(format!("invalid proxy response from pid {pid}: {error}"))
    })?;
    if response.protocol != PROTOCOL || response.pid != pid {
        return Err(ControlClientError::Refused(format!(
            "the answer of pid {pid} speaks protocol {} as pid {}, not {PROTOCOL} as native peer pid {pid}: \
             the host process runs a build that speaks another control protocol than this command",
            response.protocol, response.pid
        )));
    }
    if let Some(error) = &response.error {
        return Err(ControlClientError::Refused(format!(
            "Stado proxy owner pid {pid} refused {:?}: {error}",
            request.action
        )));
    }
    let coordinates = match &request.action {
        Action::Ensure { state, bind }
        | Action::Inspect { state, bind }
        | Action::Stop { state, bind } => Some((state, bind)),
        Action::Retire { .. }
        | Action::AdoptTransaction { .. }
        | Action::InspectTransaction { .. } => None,
    };
    if let Some((state, bind)) = coordinates {
        if response
            .proxy
            .as_ref()
            .is_some_and(|proxy| &proxy.state != state || &proxy.bind != bind)
        {
            return Err(ControlClientError::Refused(format!(
                "proxy owner pid {pid} answered for a different state file or bind"
            )));
        }
    }
    Ok(Some(response))
}
