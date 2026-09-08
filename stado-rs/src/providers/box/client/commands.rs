//! Box command, SSH-key, and interrupt verbs.
//!
//! Port of the command half of `stado/providers/box/client.py`.

use serde_json::{json, Map, Value};

use super::super::types::{jbool, jstr, BoxCommandResult, BoxError};
use super::BoxClient;

impl BoxClient {
    /// POST /boxes/{id}/commands; timeout bounded to the API's 1..=60s
    /// window (Python `ValueError` outside it).
    pub async fn execute_command(
        &self,
        box_id: &str,
        command: &str,
        cwd: &str,
        timeout_seconds: i64,
    ) -> Result<BoxCommandResult, BoxError> {
        if command.is_empty() {
            return Err(BoxError::value("Box command is required"));
        }
        if !(1..=60).contains(&timeout_seconds) {
            return Err(BoxError::value("Box command timeout is outside API bounds"));
        }
        let mut body = json!({"command": command, "timeoutSeconds": timeout_seconds});
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

    /// POST /boxes/{id}/sshkey; the key must look like an OpenSSH public
    /// key (Python `ValueError`).
    pub async fn configure_ssh_key(
        &self,
        box_id: &str,
        public_key: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        if !public_key.starts_with("ssh-") {
            return Err(BoxError::value("OpenSSH public key is required"));
        }
        let body = json!({"key": public_key});
        self.transport
            .request_json(
                "POST",
                &format!("{}/sshkey", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["sshkey.configured", "box.sshkey"],
            )
            .await
    }

    /// POST /boxes/{id}/interrupt.
    pub async fn interrupt(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "POST",
                &format!("{}/interrupt", Self::box_path(box_id)?),
                None,
                &[],
                &["box.interrupted"],
            )
            .await
    }
}
