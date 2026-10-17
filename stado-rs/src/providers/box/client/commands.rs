//! Boat command, SSH-key, and interrupt verbs.

use serde_json::{json, Map, Value};

use super::super::types::{jbool, jstr, BoxCommandResult, BoxError};
use super::BoxClient;

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
}
