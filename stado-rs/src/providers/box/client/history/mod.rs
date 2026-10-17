//! What ran in a Boat sandbox: its agent conversations, the history of its
//! commands and other processes with their stored output, and its process
//! list. The history and its output are kept by Boat and read also while
//! the sandbox is stopped; the process list of a stopped sandbox is the last
//! sample Boat kept.

use serde_json::{Map, Value};

use super::super::types::BoxError;
use super::files::segment_path;
use super::BoxClient;

/// The path of one command of a sandbox's history, by the id Boat answered.
fn command_path(box_id: &str, command: &str) -> Result<String, BoxError> {
    segment_path(
        &format!("{}/command-history", BoxClient::box_path(box_id)?),
        command,
        "a command id",
    )
}

impl BoxClient {
    /// GET /sandboxes/{id}/conversations: every conversation, most recently
    /// prompted first, with the one a prompt would continue marked current.
    pub async fn conversations(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &format!("{}/conversations", Self::box_path(box_id)?),
                None,
                &[],
                &["conversation.list"],
            )
            .await
    }

    /// GET /sandboxes/{id}/command-history: one page, newest first. `modes`
    /// is Boat's comma-separated mode filter and `query` text a command line
    /// contains; an empty one, a `limit` of `None` and an empty `cursor` are
    /// left out of the request, so Boat answers its first page of every mode.
    pub async fn command_history(
        &self,
        box_id: &str,
        modes: &str,
        query: &str,
        limit: Option<u32>,
        cursor: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        let mut parameters = vec![
            ("mode", modes.to_string()),
            ("q", query.to_string()),
            ("cursor", cursor.to_string()),
        ];
        if let Some(limit) = limit {
            parameters.push(("limit", limit.to_string()));
        }
        self.transport
            .request_json(
                "GET",
                &format!("{}/command-history", Self::box_path(box_id)?),
                None,
                &parameters,
                &["command.list"],
            )
            .await
    }

    /// GET /sandboxes/{id}/command-history/{command}: one command of the
    /// history.
    pub async fn command_info(
        &self,
        box_id: &str,
        command: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &command_path(box_id, command)?,
                None,
                &[],
                &["command.info"],
            )
            .await
    }

    /// GET /sandboxes/{id}/command-history/{command}/logs: the stored
    /// `stream` (stdout or stderr) of one command from `offset`, in
    /// `encoding`. Boat answers `nextOffset` to read on and `done` once the
    /// command ended and the read reached its end; an `offset` of `None` is
    /// left out, so the read starts where Boat starts it.
    pub async fn command_logs(
        &self,
        box_id: &str,
        command: &str,
        stream: &str,
        offset: Option<u64>,
        encoding: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        let mut parameters = vec![
            ("stream", stream.to_string()),
            ("encoding", encoding.to_string()),
        ];
        if let Some(offset) = offset {
            parameters.push(("offset", offset.to_string()));
        }
        self.transport
            .request_json(
                "GET",
                &format!("{}/logs", command_path(box_id, command)?),
                None,
                &parameters,
                &["command.logs"],
            )
            .await
    }

    /// GET /sandboxes/{id}/processes: every user-space process of a running
    /// sandbox, busiest first, or the last sample kept of a stopped one
    /// (`live` false).
    pub async fn processes(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &format!("{}/processes", Self::box_path(box_id)?),
                None,
                &[],
                &["process.list"],
            )
            .await
    }
}
