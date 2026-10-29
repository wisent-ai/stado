//! A connection this node opens to another host for a process of its own
//! host.
//!
//! macOS keeps a Local Network decision per executable, and every install
//! replaces `~/.stado/bin/stado`, so a process run from it starts undecided and
//! its connections to LAN peers fail as "No route to host" until someone at
//! the Mac answers. The node runs its own copy, which no install replaces, so
//! the connection it opens keeps the permission given once.

use super::super::secure::{Session, Stream};
use super::super::Answer;

/// Reach `port` on registry host `host`'s loopback for `peer` and carry the
/// connection to it, when `peer` is this node's own host `here`; any other
/// peer is refused, because the node opens connections for its own host
/// only.
pub(super) async fn carry<S: Stream>(
    mut session: Session<S>,
    peer: &str,
    here: &str,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let reached = if peer == here {
        super::super::dial::direct(host, port).await
    } else {
        Err(format!(
            "{here}'s node opens connections for its own host only, and {peer} is another host"
        ))
    };
    match reached {
        Ok(stream) => {
            session
                .send(&Answer::Ok {
                    host: here.to_string(),
                })
                .await?;
            super::splice(session.into_plain(), stream).await
        }
        Err(reason) => {
            session
                .send(&Answer::Refused {
                    reason: reason.clone(),
                })
                .await?;
            Err(format!("{peer}: {reason}"))
        }
    }
}
