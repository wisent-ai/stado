//! Typed operations for Box Public API v1.
//!
//! Port of `stado/providers/box/client.py`. Python subclasses the
//! transport; here [`BoxClient`] composes a [`BoxHttpTransport`]. Every
//! method validates its arguments exactly where the Python raises
//! `ValueError` ([`BoxError::Value`]) and preserves the endpoint paths,
//! expected response types, and cursor-pagination contract.

use serde_json::{json, Map, Value};

use super::http::BoxHttpTransport;
use super::types::{
    box_id_pattern, jbool, jint_or, jstr, parse_box_info, required_dict, BoxCommandResult,
    BoxError, BoxEventPage, BoxInfo, BoxLimits, BoxPromptRun, HTTP_NOT_FOUND,
};

/// Python `BoxClient`: lifecycle, command, file, event, artifact, and
/// prompt operations over the bounded transport.
#[derive(Debug, Clone)]
pub struct BoxClient {
    transport: BoxHttpTransport,
}

/// Python `int | None | object` sentinel for `update_box(ttl_seconds=...)`:
/// `NotProvided` omits the key, `Clear` sends JSON null, `Set` sends a
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtlUpdate {
    NotProvided,
    Clear,
    Set(i64),
}

impl BoxClient {
    /// Build a client with a validated transport (Python constructor
    /// defaults: `base_url=DEFAULT_BOX_API_URL`, `timeout_seconds=70`).
    pub fn new(api_key: &str, base_url: &str, timeout_seconds: f64) -> Result<Self, BoxError> {
        Ok(BoxClient {
            transport: BoxHttpTransport::new(api_key, base_url, timeout_seconds)?,
        })
    }

    /// Wrap an already-built transport (tests, custom wiring).
    pub fn from_transport(transport: BoxHttpTransport) -> Self {
        BoxClient { transport }
    }

    /// The underlying transport.
    pub fn transport(&self) -> &BoxHttpTransport {
        &self.transport
    }

    /// Python `validate_box_id` (`ValueError` on a non-conforming id).
    pub fn validate_box_id(box_id: &str) -> Result<&str, BoxError> {
        if !box_id_pattern().is_match(box_id) {
            return Err(BoxError::value("invalid Box id"));
        }
        Ok(box_id)
    }

    fn box_path(box_id: &str) -> Result<String, BoxError> {
        Ok(format!("/boxes/{}", Self::validate_box_id(box_id)?))
    }

    /// GET /limits.
    pub async fn limits(&self) -> Result<BoxLimits, BoxError> {
        let value = self
            .transport
            .request_json("GET", "/limits", None, &[], &["limits.info"])
            .await?;
        Ok(BoxLimits {
            can_start: jbool(value.get("canStart")),
            active_boxes: jint_or(value.get("activeBoxes"), 0)?,
            max_active_boxes: jint_or(value.get("maxActiveBoxes"), 0)?,
            billing_status: jstr(value.get("billingStatus")),
            blocked_reason: super::types::first_truthy_str(
                &[value.get("startBlockedReason"), value.get("blockedReason")],
                "",
            ),
            credit_balance_seconds: jint_or(value.get("creditBalanceSeconds"), 0)?,
        })
    }

    /// POST /boxes. `ttl_seconds=None` sends an explicit JSON null, like
    /// Python `{"ttlSeconds": None, "noEnv": True}`.
    pub async fn create_box(
        &self,
        ttl_seconds: Option<i64>,
        no_env: bool,
    ) -> Result<BoxInfo, BoxError> {
        let body = json!({"ttlSeconds": ttl_seconds, "noEnv": no_env});
        let value = self
            .transport
            .request_json("POST", "/boxes", Some(&body), &[], &["box.created"])
            .await?;
        parse_box_info(&value)
    }

    /// GET /boxes/{id}.
    pub async fn get_box(&self, box_id: &str) -> Result<BoxInfo, BoxError> {
        let value = self
            .transport
            .request_json(
                "GET",
                &Self::box_path(box_id)?,
                None,
                &[],
                &["box.info", "box.get"],
            )
            .await?;
        parse_box_info(&value)
    }

    /// GET /boxes with cursor pagination (Python `list_boxes`).
    pub async fn list_boxes(&self) -> Result<Vec<BoxInfo>, BoxError> {
        let mut boxes: Vec<BoxInfo> = Vec::new();
        let mut cursor = String::new();
        loop {
            let value = self
                .transport
                .request_json(
                    "GET",
                    "/boxes",
                    None,
                    &[("cursor", cursor.clone()), ("sort", "asc".to_string())],
                    &["box.list"],
                )
                .await?;
            let rows = value
                .get("boxes")
                .and_then(Value::as_array)
                .ok_or_else(|| BoxError::transport("Box list response has invalid boxes"))?;
            for row in rows {
                let row = row
                    .as_object()
                    .ok_or_else(|| BoxError::transport("Box list response has invalid boxes"))?;
                boxes.push(parse_box_info(row)?);
            }
            let page = value.get("pageInfo").and_then(Value::as_object);
            if !page.is_some_and(|p| jbool(p.get("hasMore"))) {
                return Ok(boxes);
            }
            cursor = page.map(|p| jstr(p.get("nextCursor"))).unwrap_or_default();
            if cursor.is_empty() {
                return Err(BoxError::transport(
                    "Box list pagination omitted next cursor",
                ));
            }
        }
    }

    /// PATCH /boxes/{id}; at least one field is required (Python
    /// `ValueError("Box update requires at least one field")`).
    pub async fn update_box(
        &self,
        box_id: &str,
        name: Option<&str>,
        ttl_seconds: TtlUpdate,
    ) -> Result<BoxInfo, BoxError> {
        let mut body = Map::new();
        if let Some(name) = name {
            body.insert("name".to_string(), Value::from(name));
        }
        match ttl_seconds {
            TtlUpdate::NotProvided => {}
            TtlUpdate::Clear => {
                body.insert("ttlSeconds".to_string(), Value::Null);
            }
            TtlUpdate::Set(ttl) => {
                body.insert("ttlSeconds".to_string(), Value::from(ttl));
            }
        }
        if body.is_empty() {
            return Err(BoxError::value("Box update requires at least one field"));
        }
        let value = self
            .transport
            .request_json(
                "PATCH",
                &Self::box_path(box_id)?,
                Some(&Value::Object(body)),
                &[],
                &["box.updated", "box.info"],
            )
            .await?;
        parse_box_info(&value)
    }

    /// POST /boxes/{id}/stop.
    pub async fn stop_box(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "POST",
                &format!("{}/stop", Self::box_path(box_id)?),
                None,
                &[],
                &["box.stopping", "box.action"],
            )
            .await
    }

    /// POST /boxes/{id}/resume; `no_env=None` sends no body.
    pub async fn resume_box(
        &self,
        box_id: &str,
        no_env: Option<bool>,
    ) -> Result<Map<String, Value>, BoxError> {
        let body = no_env.map(|flag| json!({"noEnv": flag}));
        self.transport
            .request_json(
                "POST",
                &format!("{}/resume", Self::box_path(box_id)?),
                body.as_ref(),
                &[],
                &["box.resuming", "box.action"],
            )
            .await
    }

    /// POST /boxes/{id}/fork; `no_env=None` sends no body.
    pub async fn fork_box(&self, box_id: &str, no_env: Option<bool>) -> Result<BoxInfo, BoxError> {
        let body = no_env.map(|flag| json!({"noEnv": flag}));
        let value = self
            .transport
            .request_json(
                "POST",
                &format!("{}/fork", Self::box_path(box_id)?),
                body.as_ref(),
                &[],
                &["box.forking"],
            )
            .await?;
        parse_box_info(&value)
    }

    /// DELETE /boxes/{id}; a 404 is an idempotent success (Python swallows
    /// `BoxAPIError` with status 404).
    pub async fn delete_box(&self, box_id: &str) -> Result<(), BoxError> {
        match self
            .transport
            .request_json(
                "DELETE",
                &Self::box_path(box_id)?,
                None,
                &[],
                &["box.deleted"],
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(BoxError::Api(api)) if api.status == HTTP_NOT_FOUND => Ok(()),
            Err(err) => Err(err),
        }
    }

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

    /// GET /boxes/{id}/files.
    pub async fn read_file(
        &self,
        box_id: &str,
        path: &str,
        encoding: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "GET",
                &format!("{}/files", Self::box_path(box_id)?),
                None,
                &[
                    ("path", path.to_string()),
                    ("encoding", encoding.to_string()),
                ],
                &["file.read"],
            )
            .await
    }

    /// PUT /boxes/{id}/files.
    pub async fn write_file(
        &self,
        box_id: &str,
        path: &str,
        content: &str,
        encoding: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        let body = json!({"path": path, "content": content, "encoding": encoding});
        self.transport
            .request_json(
                "PUT",
                &format!("{}/files", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["file.written", "file.write"],
            )
            .await
    }

    /// GET /boxes/{id}/artifacts (binary; `max_bytes` must be positive —
    /// Python `ValueError`).
    pub async fn download_artifact(
        &self,
        box_id: &str,
        path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, BoxError> {
        if max_bytes == 0 {
            return Err(BoxError::value("artifact max_bytes must be positive"));
        }
        self.transport
            .request_binary(
                "GET",
                &format!("{}/artifacts", Self::box_path(box_id)?),
                &[("path", path.to_string())],
                max_bytes,
            )
            .await
    }

    /// GET /boxes/{id}/events.
    pub async fn list_events(
        &self,
        box_id: &str,
        cursor: &str,
        limit: i64,
        sort: &str,
        event_type: &str,
    ) -> Result<BoxEventPage, BoxError> {
        let value = self
            .transport
            .request_json(
                "GET",
                &format!("{}/events", Self::box_path(box_id)?),
                None,
                &[
                    ("cursor", cursor.to_string()),
                    ("limit", limit.to_string()),
                    ("sort", sort.to_string()),
                    ("type", event_type.to_string()),
                ],
                &["events.list"],
            )
            .await?;
        let events_value = value
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| BoxError::transport("Box events response has invalid events"))?;
        let mut events = Vec::with_capacity(events_value.len());
        for event in events_value {
            events.push(
                event
                    .as_object()
                    .cloned()
                    .ok_or_else(|| BoxError::transport("Box events response has invalid events"))?,
            );
        }
        let page = value.get("pageInfo").and_then(Value::as_object);
        Ok(BoxEventPage {
            events,
            next_cursor: page.map(|p| jstr(p.get("nextCursor"))).unwrap_or_default(),
            has_more: page.is_some_and(|p| jbool(p.get("hasMore"))),
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

    /// POST /boxes/{id}/prompt. The response must carry a `promptRun` dict
    /// whose id matches the top-level `promptId` (when present).
    pub async fn prompt(
        &self,
        box_id: &str,
        prompt: &str,
        provider: &str,
        model: &str,
        reasoning_effort: &str,
    ) -> Result<BoxPromptRun, BoxError> {
        let mut body = json!({"provider": provider, "prompt": prompt});
        if !model.is_empty() {
            body["model"] = Value::from(model);
        }
        if !reasoning_effort.is_empty() {
            body["reasoningEffort"] = Value::from(reasoning_effort);
        }
        let value = self
            .transport
            .request_json(
                "POST",
                &format!("{}/prompt", Self::box_path(box_id)?),
                Some(&body),
                &[],
                &["prompt.queued"],
            )
            .await?;
        let prompt_run = required_dict(
            value.get("promptRun").cloned().unwrap_or(Value::Null),
            "prompt",
        )
        .map_err(|_| BoxError::transport("Box prompt response omitted promptRun"))?;
        let prompt_id = super::types::first_truthy_str(
            &[value.get("promptId"), prompt_run.get("promptId")],
            "",
        );
        if prompt_id.is_empty() || jstr(prompt_run.get("promptId")) != prompt_id {
            return Err(BoxError::transport(
                "Box prompt response has an invalid prompt id",
            ));
        }
        Ok(BoxPromptRun {
            prompt_id,
            status: {
                let status = jstr(prompt_run.get("status"));
                if status.is_empty() {
                    "queued".to_string()
                } else {
                    status
                }
            },
            done: jbool(prompt_run.get("done")),
            raw: prompt_run,
        })
    }

    /// GET /boxes/{id}/prompts/{prompt_id}.
    pub async fn prompt_status(
        &self,
        box_id: &str,
        prompt_id: &str,
    ) -> Result<BoxPromptRun, BoxError> {
        let value = self
            .transport
            .request_json(
                "GET",
                &format!("{}/prompts/{prompt_id}", Self::box_path(box_id)?),
                None,
                &[],
                &["prompt.run"],
            )
            .await?;
        let prompt_run = required_dict(
            value.get("promptRun").cloned().unwrap_or(Value::Null),
            "prompt",
        )
        .map_err(|_| BoxError::transport("Box prompt status omitted promptRun"))?;
        let returned_id = jstr(prompt_run.get("promptId"));
        if returned_id != prompt_id {
            return Err(BoxError::transport("Box prompt status id mismatch"));
        }
        let status = jstr(prompt_run.get("status"));
        if !["sending", "queued", "running", "finished", "failed"].contains(&status.as_str()) {
            return Err(BoxError::transport("Box prompt status is invalid"));
        }
        Ok(BoxPromptRun {
            prompt_id: prompt_id.to_string(),
            status,
            done: jbool(prompt_run.get("done")),
            raw: prompt_run,
        })
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

    /// Python `wait_for_state`: poll `get_box` until the state enters
    /// `states` or the deadline lapses (transport error, like Python).
    pub async fn wait_for_state(
        &self,
        box_id: &str,
        states: &[&str],
        deadline_seconds: f64,
        poll_seconds: f64,
    ) -> Result<BoxInfo, BoxError> {
        let deadline = tokio::time::Instant::now()
            + tokio::time::Duration::from_secs_f64(deadline_seconds.max(0.0));
        loop {
            let info = self.get_box(box_id).await?;
            if states.contains(&info.state.as_str()) {
                return Ok(info);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(BoxError::transport("timed out waiting for Box state"));
            }
            tokio::time::sleep(remaining.min(tokio::time::Duration::from_secs_f64(poll_seconds)))
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{http_response, mock_http};
    use serde_json::json;

    const BX: &str = "bx_2abcdefg";

    async fn client_for(responses: Vec<String>) -> (crate::testutil::MockHttp, BoxClient) {
        let server = mock_http(responses).await;
        let transport = BoxHttpTransport::new_for_test("box_testkey", &server.base_url, 5.0);
        (server, BoxClient::from_transport(transport))
    }

    #[test]
    fn validate_box_id_matches_python() {
        assert_eq!(BoxClient::validate_box_id(BX).unwrap(), BX);
        let err = BoxClient::validate_box_id("nope").unwrap_err();
        assert_eq!(err.to_string(), "invalid Box id");
        let err = BoxClient::validate_box_id("").unwrap_err();
        assert_eq!(err.to_string(), "invalid Box id");
    }

    #[tokio::test]
    async fn client_side_argument_validation() {
        // These raise before any network I/O, like Python ValueError.
        let (server, client) = client_for(vec![]).await;
        let err = client.execute_command(BX, "", "", 30).await.unwrap_err();
        assert_eq!(err.to_string(), "Box command is required");
        let err = client.execute_command(BX, "ls", "", 0).await.unwrap_err();
        assert_eq!(err.to_string(), "Box command timeout is outside API bounds");
        let err = client.execute_command(BX, "ls", "", 61).await.unwrap_err();
        assert_eq!(err.to_string(), "Box command timeout is outside API bounds");
        let err = client
            .update_box(BX, None, TtlUpdate::NotProvided)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Box update requires at least one field");
        let err = client.download_artifact(BX, "/a", 0).await.unwrap_err();
        assert_eq!(err.to_string(), "artifact max_bytes must be positive");
        let err = client.configure_ssh_key(BX, "not-a-key").await.unwrap_err();
        assert_eq!(err.to_string(), "OpenSSH public key is required");
        server.stop();
    }

    #[tokio::test]
    async fn limits_parsing() {
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "limits.info", "canStart": false, "activeBoxes": 3,
                "maxActiveBoxes": 5, "billingStatus": "overdue", "startBlockedReason": "pay up",
                "creditBalanceSeconds": 120}"#,
        )])
        .await;
        let limits = client.limits().await.unwrap();
        assert_eq!(
            limits,
            BoxLimits {
                can_start: false,
                active_boxes: 3,
                max_active_boxes: 5,
                billing_status: "overdue".into(),
                blocked_reason: "pay up".into(),
                credit_balance_seconds: 120,
            }
        );
        server.stop();

        // blockedReason fallback when startBlockedReason is absent.
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "limits.info", "canStart": true, "blockedReason": "other"}"#,
        )])
        .await;
        let limits = client.limits().await.unwrap();
        assert!(limits.can_start);
        assert_eq!(limits.blocked_reason, "other");
        assert_eq!(limits.active_boxes, 0);
        server.stop();
    }

    #[tokio::test]
    async fn create_box_sends_null_ttl_and_parses_envelope() {
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.created",
                "box": {"id": "bx_2abcdefg", "state": "provisioning"}}"#,
        )])
        .await;
        let info = client.create_box(None, true).await.unwrap();
        assert_eq!(info.box_id, BX);
        assert_eq!(info.state, "provisioning");
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].ends_with(r#"{"ttlSeconds":null,"noEnv":true}"#),
            "{}",
            requests[0]
        );
        server.stop();
    }

    #[tokio::test]
    async fn list_boxes_paginates_with_cursor() {
        let page1 = http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.list",
                "boxes": [{"id": "bx_2abcdefg", "state": "ready"}],
                "pageInfo": {"hasMore": true, "nextCursor": "cur/2"}}"#,
        );
        let page2 = http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.list",
                "boxes": [{"id": "bx_3bcdefgh", "state": "idle"}],
                "pageInfo": {"hasMore": false}}"#,
        );
        let (server, client) = client_for(vec![page1, page2]).await;
        let boxes = client.list_boxes().await.unwrap();
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[1].box_id, "bx_3bcdefgh");
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0].starts_with("GET /boxes?sort=asc "),
            "{}",
            requests[0]
        );
        // Cursor is quote_plus-encoded on the wire.
        assert!(
            requests[1].starts_with("GET /boxes?cursor=cur%2F2&sort=asc "),
            "{}",
            requests[1]
        );
        server.stop();

        // hasMore without a cursor is a transport failure, like Python.
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.list", "boxes": [],
                "pageInfo": {"hasMore": true}}"#,
        )])
        .await;
        let err = client.list_boxes().await.unwrap_err();
        assert!(err.to_string().contains("omitted next cursor"), "{err}");
        server.stop();

        // Non-list boxes payload is rejected.
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.list", "boxes": {"x": 1}}"#,
        )])
        .await;
        let err = client.list_boxes().await.unwrap_err();
        assert!(err.to_string().contains("invalid boxes"), "{err}");
        server.stop();
    }

    #[tokio::test]
    async fn delete_box_swallows_404() {
        let (server, client) = client_for(vec![http_response(
            404,
            "Not Found",
            r#"{"code": "box_not_found", "message": "gone"}"#,
        )])
        .await;
        client.delete_box(BX).await.unwrap();
        server.stop();

        // Other statuses still raise.
        let (server, client) = client_for(vec![http_response(500, "ISE", "{}")]).await;
        let err = client.delete_box(BX).await.unwrap_err();
        assert!(matches!(err, BoxError::Api(_)), "{err:?}");
        server.stop();
    }

    #[tokio::test]
    async fn execute_command_parses_result() {
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "command.finished", "success": true, "exitCode": 0,
                "stdout": "hi\n", "stdoutTruncated": false, "timedOut": false}"#,
        )])
        .await;
        let result = client
            .execute_command(BX, "echo hi", "/work", 30)
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "hi\n");
        assert_eq!(result.signal, "");
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].ends_with(r#"{"command":"echo hi","timeoutSeconds":30,"cwd":"/work"}"#),
            "{}",
            requests[0]
        );
        server.stop();
    }

    #[tokio::test]
    async fn update_box_body_variants() {
        // Name only.
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.updated", "box": {"id": "bx_2abcdefg", "name": "n2"}}"#,
        )])
        .await;
        client
            .update_box(BX, Some("n2"), TtlUpdate::NotProvided)
            .await
            .unwrap();
        assert!(server.requests.lock().unwrap()[0].ends_with(r#"{"name":"n2"}"#));
        server.stop();

        // TTL clear -> explicit null.
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.updated", "box": {"id": "bx_2abcdefg"}}"#,
        )])
        .await;
        client.update_box(BX, None, TtlUpdate::Clear).await.unwrap();
        assert!(server.requests.lock().unwrap()[0].ends_with(r#"{"ttlSeconds":null}"#));
        server.stop();
    }

    #[tokio::test]
    async fn prompt_round_trip_and_validation() {
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "prompt.queued", "promptId": "pr-1",
                "promptRun": {"promptId": "pr-1", "status": "queued", "done": false}}"#,
        )])
        .await;
        let run = client
            .prompt(BX, "do it", "claude", "opus", "high")
            .await
            .unwrap();
        assert_eq!(run.prompt_id, "pr-1");
        assert_eq!(run.status, "queued");
        assert!(!run.done);
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].ends_with(
                r#"{"provider":"claude","prompt":"do it","model":"opus","reasoningEffort":"high"}"#
            ),
            "{}",
            requests[0]
        );
        server.stop();

        // Missing promptRun.
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "prompt.queued", "promptId": "pr-1"}"#,
        )])
        .await;
        let err = client.prompt(BX, "p", "claude", "", "").await.unwrap_err();
        assert!(err.to_string().contains("omitted promptRun"), "{err}");
        server.stop();

        // Id mismatch.
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "prompt.queued", "promptId": "pr-1",
                "promptRun": {"promptId": "pr-2"}}"#,
        )])
        .await;
        let err = client.prompt(BX, "p", "claude", "", "").await.unwrap_err();
        assert!(err.to_string().contains("invalid prompt id"), "{err}");
        server.stop();
    }

    #[tokio::test]
    async fn prompt_status_validates_id_and_state() {
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "prompt.run",
                "promptRun": {"promptId": "pr-1", "status": "finished", "done": true}}"#,
        )])
        .await;
        let run = client.prompt_status(BX, "pr-1").await.unwrap();
        assert_eq!(run.status, "finished");
        assert!(run.done);
        server.stop();

        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "prompt.run",
                "promptRun": {"promptId": "pr-9", "status": "running"}}"#,
        )])
        .await;
        let err = client.prompt_status(BX, "pr-1").await.unwrap_err();
        assert!(err.to_string().contains("id mismatch"), "{err}");
        server.stop();

        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "prompt.run",
                "promptRun": {"promptId": "pr-1", "status": "weird"}}"#,
        )])
        .await;
        let err = client.prompt_status(BX, "pr-1").await.unwrap_err();
        assert!(err.to_string().contains("status is invalid"), "{err}");
        server.stop();
    }

    #[tokio::test]
    async fn events_and_files_and_artifact_paths() {
        let (server, client) = client_for(vec![
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "events.list", "events": [{"kind": "start"}],
                    "pageInfo": {"hasMore": true, "nextCursor": "c9"}}"#,
            ),
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "file.read", "content": "data"}"#,
            ),
            http_response(200, "OK", r#"{"ok": true, "type": "file.written"}"#),
            http_response(200, "OK", "BINARY_PAYLOAD"),
        ])
        .await;
        let page = client.list_events(BX, "", 100, "asc", "").await.unwrap();
        assert!(page.has_more);
        assert_eq!(page.next_cursor, "c9");
        assert_eq!(page.events[0]["kind"], json!("start"));

        let read = client.read_file(BX, "/tmp/x", "utf8").await.unwrap();
        assert_eq!(read["content"], json!("data"));
        client
            .write_file(BX, "/tmp/x", "data", "utf8")
            .await
            .unwrap();
        let bytes = client
            .download_artifact(BX, "/out.tar", 1024)
            .await
            .unwrap();
        assert_eq!(bytes, b"BINARY_PAYLOAD");

        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].starts_with("GET /boxes/bx_2abcdefg/events?limit=100&sort=asc "),
            "{}",
            requests[0]
        );
        assert!(
            requests[1].starts_with("GET /boxes/bx_2abcdefg/files?path=%2Ftmp%2Fx&encoding=utf8 "),
            "{}",
            requests[1]
        );
        assert!(
            requests[2].starts_with("PUT /boxes/bx_2abcdefg/files "),
            "{}",
            requests[2]
        );
        assert!(
            requests[3].starts_with("GET /boxes/bx_2abcdefg/artifacts?path=%2Fout.tar "),
            "{}",
            requests[3]
        );
        server.stop();
    }

    #[tokio::test]
    async fn stop_resume_fork_interrupt_hit_their_paths() {
        let (server, client) = client_for(vec![
            http_response(200, "OK", r#"{"ok": true, "type": "box.stopping"}"#),
            http_response(200, "OK", r#"{"ok": true, "type": "box.resuming"}"#),
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "box.forking", "box": {"id": "bx_9zxywvut"}}"#,
            ),
            http_response(200, "OK", r#"{"ok": true, "type": "box.interrupted"}"#),
        ])
        .await;
        client.stop_box(BX).await.unwrap();
        client.resume_box(BX, Some(true)).await.unwrap();
        let forked = client.fork_box(BX, None).await.unwrap();
        assert_eq!(forked.box_id, "bx_9zxywvut");
        client.interrupt(BX).await.unwrap();
        let requests = server.requests.lock().unwrap().clone();
        assert!(
            requests[0].starts_with("POST /boxes/bx_2abcdefg/stop "),
            "{}",
            requests[0]
        );
        assert!(
            requests[1].starts_with("POST /boxes/bx_2abcdefg/resume "),
            "{}",
            requests[1]
        );
        assert!(
            requests[1].ends_with(r#"{"noEnv":true}"#),
            "{}",
            requests[1]
        );
        assert!(
            requests[2].starts_with("POST /boxes/bx_2abcdefg/fork "),
            "{}",
            requests[2]
        );
        assert!(
            requests[3].starts_with("POST /boxes/bx_2abcdefg/interrupt "),
            "{}",
            requests[3]
        );
        server.stop();
    }

    #[tokio::test]
    async fn wait_for_state_times_out_like_python() {
        let (server, client) = client_for(vec![
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "provisioning"}}"#,
            ),
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "ready"}}"#,
            ),
        ])
        .await;
        let info = client
            .wait_for_state(BX, &["ready"], 5.0, 0.01)
            .await
            .unwrap();
        assert_eq!(info.state, "ready");
        server.stop();

        // Deadline with no matching state -> transport error.
        let (server, client) = client_for(vec![http_response(
            200,
            "OK",
            r#"{"ok": true, "type": "box.info", "box": {"id": "bx_2abcdefg", "state": "provisioning"}}"#,
        )])
        .await;
        let err = client
            .wait_for_state(BX, &["ready"], 0.0, 0.01)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("timed out waiting for Box state"),
            "{err}"
        );
        server.stop();
    }
}
