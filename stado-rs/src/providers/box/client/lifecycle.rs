//! Boat sandbox lifecycle verbs: limits, create, fetch, list, update, stop,
//! recover, resume, fork and delete.

use serde_json::{json, Map, Value};

use super::super::types::{
    jbool, jstr, parse_box_info, parse_sandbox_envelope, BoxError, BoxInfo, BoxLimits,
    HTTP_NOT_FOUND,
};
use super::{BoxClient, TtlUpdate};

/// The header a permanent deletion carries, equal to the sandbox id it
/// deletes; Boat refuses the deletion without it.
const CONFIRM_DELETE_HEADER: &str = "X-Ascii-Confirm-Delete";

/// One integer field Boat's answer is documented to carry, refused by name
/// when it is absent or not an integer.
fn required_integer(
    payload: &Map<String, Value>,
    key: &str,
    answer: &str,
) -> Result<i64, BoxError> {
    match payload.get(key) {
        Some(value) => value.as_i64().ok_or_else(|| {
            BoxError::transport(format!(
                "Boat answered {answer} with {key} {value}, which is not an integer"
            ))
        }),
        None => Err(BoxError::transport(format!(
            "Boat answered {answer} without {key}"
        ))),
    }
}

impl BoxClient {
    /// GET /limits.
    pub async fn limits(&self) -> Result<BoxLimits, BoxError> {
        let value = self
            .transport
            .request_json("GET", "/limits", None, &[], &["limits.info"])
            .await?;
        Ok(BoxLimits {
            can_start: jbool(value.get("canStart")),
            active_boxes: required_integer(&value, "activeSandboxes", "GET /limits")?,
            max_active_boxes: required_integer(&value, "maxActiveSandboxes", "GET /limits")?,
            billing_status: jstr(value.get("billingStatus")),
            blocked_reason: super::super::types::first_truthy_str(
                &[value.get("startBlockedReason"), value.get("blockedReason")],
                "",
            ),
        })
    }

    /// POST /sandboxes. `ttl_seconds=None` sends an explicit JSON null, which
    /// Boat reads as "never stop by itself"; `from` names the named snapshot
    /// the sandbox starts from and `environment` the environment it starts
    /// with, and without either the request names none.
    pub async fn create_box(
        &self,
        ttl_seconds: Option<i64>,
        no_env: bool,
        from: Option<&str>,
        environment: Option<&str>,
    ) -> Result<BoxInfo, BoxError> {
        let mut body = json!({"ttlSeconds": ttl_seconds, "noEnv": no_env});
        if let Some(snapshot) = from {
            body["from"] = Value::from(snapshot);
        }
        if let Some(environment) = environment {
            body["environment"] = Value::from(environment);
        }
        let value = self
            .transport
            .request_json("POST", "/sandboxes", Some(&body), &[], &["sandbox.created"])
            .await?;
        parse_sandbox_envelope(&value)
    }

    /// GET /sandboxes/{id}.
    pub async fn get_box(&self, box_id: &str) -> Result<BoxInfo, BoxError> {
        let value = self
            .transport
            .request_json(
                "GET",
                &Self::box_path(box_id)?,
                None,
                &[],
                &["sandbox.info"],
            )
            .await?;
        parse_sandbox_envelope(&value)
    }

    /// GET /sandboxes, every page. An answer without `pageInfo` is the only
    /// page there is.
    pub async fn list_boxes(&self) -> Result<Vec<BoxInfo>, BoxError> {
        let mut sandboxes: Vec<BoxInfo> = Vec::new();
        let mut cursor = String::new();
        loop {
            let value = self
                .transport
                .request_json(
                    "GET",
                    "/sandboxes",
                    None,
                    &[("cursor", cursor.clone()), ("sort", "asc".to_string())],
                    &["sandbox.list"],
                )
                .await?;
            let rows = match value.get("sandboxes") {
                Some(Value::Array(rows)) => rows,
                Some(other) => {
                    return Err(BoxError::transport(format!(
                        "Boat answered GET /sandboxes with sandboxes that are not a list: {other}"
                    )))
                }
                None => {
                    return Err(BoxError::transport(
                        "Boat answered GET /sandboxes without sandboxes",
                    ))
                }
            };
            for row in rows {
                let Value::Object(record) = row else {
                    return Err(BoxError::transport(format!(
                        "Boat answered GET /sandboxes with a sandbox that is not an object: {row}"
                    )));
                };
                sandboxes.push(parse_box_info(record)?);
            }
            let Some(page) = value.get("pageInfo").and_then(Value::as_object) else {
                return Ok(sandboxes);
            };
            if !jbool(page.get("hasMore")) {
                return Ok(sandboxes);
            }
            cursor = jstr(page.get("nextCursor"));
            if cursor.is_empty() {
                return Err(BoxError::transport(
                    "Boat answered GET /sandboxes with more pages and no next cursor",
                ));
            }
        }
    }

    /// PATCH /sandboxes/{id}; at least one field is required.
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
            return Err(BoxError::value(
                "a Boat sandbox update names at least a name or an auto-stop",
            ));
        }
        let value = self
            .transport
            .request_json(
                "PATCH",
                &Self::box_path(box_id)?,
                Some(&Value::Object(body)),
                &[],
                &["sandbox.updated", "sandbox.info"],
            )
            .await?;
        parse_sandbox_envelope(&value)
    }

    /// POST /sandboxes/{id}/stop. Without `force` the request sends no body
    /// and Boat saves the disk first; with it Boat stops even though that
    /// save fails, keeping the last successful snapshot.
    pub async fn stop_box(
        &self,
        box_id: &str,
        force: bool,
    ) -> Result<Map<String, Value>, BoxError> {
        let body = force.then(|| json!({"force": true}));
        self.transport
            .request_json(
                "POST",
                &format!("{}/stop", Self::box_path(box_id)?),
                body.as_ref(),
                &[],
                &["sandbox.stopping"],
            )
            .await
    }

    /// POST /sandboxes/{id}/recover: power-cycle the machine of a sandbox
    /// that stopped answering, leaving its disk as it is. Boat refuses it
    /// with a code saying why (the sandbox answers, a recover is too recent,
    /// there is no machine, the machine cannot be power-cycled).
    pub async fn recover_box(&self, box_id: &str) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json(
                "POST",
                &format!("{}/recover", Self::box_path(box_id)?),
                None,
                &[],
                &["sandbox.recovering"],
            )
            .await
    }

    /// POST /sandboxes/{id}/resume; `no_env=None` sends no body.
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
                &["sandbox.resuming"],
            )
            .await
    }

    /// POST /sandboxes/{id}/fork; `no_env=None` sends no body.
    pub async fn fork_box(&self, box_id: &str, no_env: Option<bool>) -> Result<BoxInfo, BoxError> {
        let body = no_env.map(|flag| json!({"noEnv": flag}));
        let value = self
            .transport
            .request_json(
                "POST",
                &format!("{}/fork", Self::box_path(box_id)?),
                body.as_ref(),
                &[],
                &["sandbox.forking"],
            )
            .await?;
        parse_sandbox_envelope(&value)
    }

    /// DELETE /sandboxes/{id}, confirmed with the sandbox's own id. Boat
    /// accepts the deletion and erases the data in the background; a
    /// sandbox Boat no longer holds is already deleted.
    pub async fn delete_box(&self, box_id: &str) -> Result<(), BoxError> {
        let path = Self::box_path(box_id)?;
        match self
            .transport
            .request_json_headed(
                "DELETE",
                &path,
                None,
                &[],
                &[(CONFIRM_DELETE_HEADER, box_id)],
                &["deletion.operation", "sandbox.deleting"],
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(BoxError::Api(api)) if api.status == HTTP_NOT_FOUND => Ok(()),
            Err(err) => Err(err),
        }
    }
}
