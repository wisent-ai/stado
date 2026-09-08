//! Box lifecycle verbs: limits, create, fetch, list, update, and release.
//!
//! Port of the lifecycle half of `stado/providers/box/client.py`.

use serde_json::{json, Map, Value};

use super::super::types::{
    jbool, jint_or, jstr, parse_box_info, BoxError, BoxInfo, BoxLimits, HTTP_NOT_FOUND,
};
use super::{BoxClient, TtlUpdate};

impl BoxClient {
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
            blocked_reason: super::super::types::first_truthy_str(
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
