//! Boat command, detached command, SSH-key, interrupt, public URL, desktop
//! URL, signed file URL, sharing and usage verbs.

use serde_json::{json, Map, Value};

use super::super::types::{jbool, jstr, parse_sandbox_envelope, BoxCommandResult, BoxError};
use super::{BoxClient, FileUrl};

impl BoxClient {
    /// POST /sandboxes/{id}/commands. The commands Stado sends return at
    /// once (launch in the background, signal a group), so the request names
    /// no limit of its own; the work they start runs detached.
    pub async fn execute_command(
        &self,
        box_id: &str,
        command: &str,
        cwd: &str,
    ) -> Result<BoxCommandResult, BoxError> {
        if command.is_empty() {
            return Err(BoxError::value("a Boat sandbox command is required"));
        }
        let mut body = json!({"command": command});
        if !cwd.is_empty() {
            body["cwd"] = Value::from(cwd);
        }
        let value = self
            .transport
            .request_json(
                "POST",
                &format!("{}/commands", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["command.finished"],
            )
            .await?;
        Ok(BoxCommandResult {
            success: jbool(value.get("success")),
            // Python keeps the value only when it is an int instance.
            exit_code: value.get("exitCode").and_then(Value::as_i64),
            signal: jstr(value.get("signal")),
            stdout: jstr(value.get("stdout")),
            stderr: jstr(value.get("stderr")),
            stdout_truncated: jbool(value.get("stdoutTruncated")),
            stderr_truncated: jbool(value.get("stderrTruncated")),
            timed_out: jbool(value.get("timedOut")),
        })
    }

    /// POST /sandboxes/{id}/sshkey; the key must look like an OpenSSH public
    /// key.
    pub async fn configure_ssh_key(
        &self,
        box_id: &str,
        public_key: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        if !public_key.starts_with("ssh-") {
            return Err(BoxError::value(
                "an OpenSSH public key (ssh-...) is required for a Boat sandbox",
            ));
        }
        let body = json!({"key": public_key});
        self.transport
            .request_json(
                "POST",
                &format!("{}/sshkey", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["ssh_key.configured"],
            )
            .await
    }

    /// POST /sandboxes/{id}/interrupt: every conversation on the sandbox.
    pub async fn interrupt(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "POST",
                &format!("{}/interrupt", Self::box_path(box_id)?),
                None,
                &[],
                &["sandbox.interrupted"],
            )
            .await
    }

    /// POST /sandboxes/{id}/host: a stable HTTPS URL for `port` inside the
    /// sandbox, gated by a token unless `public`; `localhost` says the
    /// service listens on the loopback address only. Boat answers the same
    /// URL for the same port again.
    pub async fn host_port(
        &self,
        box_id: &str,
        port: u16,
        public: bool,
        localhost: bool,
    ) -> Result<Map<String, Value>, BoxError> {
        let body = json!({"port": port, "public": public, "localhost": localhost});
        self.transport
            .request_json(
                "POST",
                &format!("{}/host", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["port.hosted"],
            )
            .await
    }

    /// POST /sandboxes/{id}/commands with `detached`: start `command` in the
    /// background and answer its process id at once; its output goes to a
    /// log on the sandbox, read with [`Self::command_status`].
    pub async fn start_command(
        &self,
        box_id: &str,
        command: &str,
        cwd: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        if command.is_empty() {
            return Err(BoxError::value("a Boat sandbox command is required"));
        }
        let mut body = json!({"command": command, "detached": true});
        if !cwd.is_empty() {
            body["cwd"] = Value::from(cwd);
        }
        self.transport
            .request_json(
                "POST",
                &format!("{}/commands", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["command.started"],
            )
            .await
    }

    /// GET /sandboxes/{id}/commands/{process}: whether a detached command
    /// runs, its exit code and its recent log.
    pub async fn command_status(
        &self,
        box_id: &str,
        process: i64,
    ) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &format!("{}/commands/{process}", Self::box_path(box_id)?),
                None,
                &[],
                &["command.status"],
            )
            .await
    }

    /// DELETE /sandboxes/{id}/commands/{process}: stop a detached command.
    pub async fn stop_command(
        &self,
        box_id: &str,
        process: i64,
    ) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "DELETE",
                &format!("{}/commands/{process}", Self::box_path(box_id)?),
                None,
                &[],
                &["command.stopped"],
            )
            .await
    }

    /// GET /sandboxes/{id}/usage: what the sandbox has run and cost.
    pub async fn usage(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &format!("{}/usage", Self::box_path(box_id)?),
                None,
                &[],
                &["sandbox.usage"],
            )
            .await
    }

    /// POST /sandboxes/{id}/desktop: the URL that streams the sandbox's
    /// desktop. It carries a secret; whoever has it sees the desktop.
    pub async fn desktop_url(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "POST",
                &format!("{}/desktop", Self::box_path(box_id)?),
                Some(&json!({})),
                &[],
                &["desktop.url"],
            )
            .await
    }

    /// POST /sandboxes/{id}/files/download-url or upload-url: a URL the
    /// sandbox serves itself for the file at `path`, which a plain GET
    /// downloads whole or a plain PUT of the bytes writes, with no size limit
    /// and no key. `ttl_seconds` of `None` leaves its lifetime to Boat. The
    /// URL is a secret until it expires.
    pub async fn file_url(
        &self,
        box_id: &str,
        path: &str,
        ttl_seconds: Option<i64>,
        operation: FileUrl,
    ) -> Result<Map<String, Value>, BoxError> {
        if path.is_empty() {
            return Err(BoxError::value("a sandbox path is required for a file URL"));
        }
        let mut body = json!({"path": path});
        if let Some(ttl) = ttl_seconds {
            body["ttlSeconds"] = Value::from(ttl);
        }
        let (segment, answer) = match operation {
            FileUrl::Download => ("download-url", "file.download_url"),
            FileUrl::Upload => ("upload-url", "file.upload_url"),
        };
        self.transport
            .request_json(
                "POST",
                &format!("{}/files/{segment}", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &[answer],
            )
            .await
    }

    /// POST /sandboxes/{id}/share: every member of the organization that
    /// pays for the sandbox may use it, and the creator's logins are wiped
    /// from it at its next start. One-way. The answer is checked for the
    /// sandbox it names and for `restartRequired`, true while a running
    /// sandbox still holds the creator's logins in memory.
    pub async fn share_box(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        let path = format!("{}/share", Self::box_path(box_id)?);
        let answer = self.transport.request_object("POST", &path, None).await?;
        parse_sandbox_envelope(&answer)?;
        match answer.get("restartRequired") {
            Some(Value::Bool(_)) => Ok(answer),
            other => Err(BoxError::transport(format!(
                "Boat answered POST {path} without a boolean restartRequired: {other:?}"
            ))),
        }
    }
}
