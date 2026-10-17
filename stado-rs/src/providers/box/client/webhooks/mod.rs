//! Boat webhooks: account-wide HTTPS endpoints Boat posts sandbox lifecycle
//! events to (ready, error, archived, hydrated, degraded, recovered). The
//! signing secret an endpoint verifies deliveries with is answered only when
//! the endpoint is created or its secret is rotated. Boat's OpenAPI document
//! names no answer type for most of these operations, so each answer is
//! checked for the field it documents instead.

use serde_json::{json, Map, Value};

use super::super::types::BoxError;
use super::files::segment_path;
use super::BoxClient;

/// The path of one webhook by the id Boat answered for it.
fn webhook_path(webhook: &str) -> Result<String, BoxError> {
    segment_path("/webhooks", webhook, "a webhook id")
}

/// `answer` when it carries `key` holding the JSON kind `holds` checks,
/// else a refusal naming the operation and the missing field.
fn carrying(
    answer: Map<String, Value>,
    key: &str,
    holds: fn(&Value) -> bool,
    operation: &str,
) -> Result<Map<String, Value>, BoxError> {
    match answer.get(key) {
        Some(value) if holds(value) => Ok(answer),
        other => Err(BoxError::transport(format!(
            "Boat answered {operation} without the {key} it documents: {other:?}"
        ))),
    }
}

impl BoxClient {
    /// GET /webhooks: every endpoint, without its signing secret.
    pub async fn webhooks(&self) -> Result<Map<String, Value>, BoxError> {
        let answer = self
            .transport
            .request_json("GET", "/webhooks", None, &[], &[])
            .await?;
        carrying(answer, "webhooks", Value::is_array, "GET /webhooks")
    }

    /// POST /webhooks: register `url` for `events`, under `name` when one is
    /// given. The answer carries the signing secret, which Boat never shows
    /// again.
    pub async fn create_webhook(
        &self,
        name: Option<&str>,
        url: &str,
        events: &[&str],
    ) -> Result<Map<String, Value>, BoxError> {
        if url.is_empty() || events.is_empty() {
            return Err(BoxError::value(
                "a Boat webhook needs an endpoint URL and at least one event",
            ));
        }
        let mut body = json!({"url": url, "events": events});
        if let Some(name) = name {
            body["name"] = Value::from(name);
        }
        let answer = self
            .transport
            .request_json("POST", "/webhooks", Some(&body), &[], &["webhook.created"])
            .await?;
        let answer = carrying(answer, "webhook", Value::is_object, "POST /webhooks")?;
        carrying(answer, "secret", Value::is_string, "POST /webhooks")
    }

    /// GET /webhooks/{id}: one endpoint, without its signing secret.
    pub async fn webhook(&self, webhook: &str) -> Result<Map<String, Value>, BoxError> {
        let path = webhook_path(webhook)?;
        let answer = self
            .transport
            .request_json("GET", &path, None, &[], &[])
            .await?;
        carrying(answer, "webhook", Value::is_object, &format!("GET {path}"))
    }

    /// PATCH /webhooks/{id}: change the name, the endpoint URL or the
    /// events, keeping the signing secret; at least one is required.
    pub async fn update_webhook(
        &self,
        webhook: &str,
        name: Option<&str>,
        url: Option<&str>,
        events: &[&str],
    ) -> Result<Map<String, Value>, BoxError> {
        let mut body = Map::new();
        if let Some(name) = name {
            body.insert("name".into(), Value::from(name));
        }
        if let Some(url) = url {
            body.insert("url".into(), Value::from(url));
        }
        if !events.is_empty() {
            body.insert("events".into(), json!(events));
        }
        if body.is_empty() {
            return Err(BoxError::value(
                "a webhook update needs a name, a URL or events",
            ));
        }
        let path = webhook_path(webhook)?;
        let answer = self
            .transport
            .request_json("PATCH", &path, Some(&Value::Object(body)), &[], &[])
            .await?;
        carrying(
            answer,
            "webhook",
            Value::is_object,
            &format!("PATCH {path}"),
        )
    }

    /// DELETE /webhooks/{id}: the endpoint and its queued deliveries; one
    /// already in flight can still arrive.
    pub async fn delete_webhook(&self, webhook: &str) -> Result<Map<String, Value>, BoxError> {
        let path = webhook_path(webhook)?;
        let answer = self
            .transport
            .request_json("DELETE", &path, None, &[], &[])
            .await?;
        carrying(answer, "id", Value::is_string, &format!("DELETE {path}"))
    }

    /// POST /webhooks/{id}/rotate: a new signing secret, answered only now;
    /// Boat briefly accepts the old one for deliveries already in flight.
    pub async fn rotate_webhook(&self, webhook: &str) -> Result<Map<String, Value>, BoxError> {
        let path = format!("{}/rotate", webhook_path(webhook)?);
        let answer = self
            .transport
            .request_json("POST", &path, None, &[], &[])
            .await?;
        carrying(answer, "secret", Value::is_string, &format!("POST {path}"))
    }
}
