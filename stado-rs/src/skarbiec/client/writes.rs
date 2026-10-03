//! The owner acts: writing an item and deleting one. Both go to the store,
//! never to `PUT /v1/items`. The one consumer write is `rotate_field`: one
//! field of an existing owner item, under the consumer's own `rotate` grant.

use serde_json::{json, Value};

use super::super::SkarbiecError;
use super::Client;

impl Client {
    /// Write one item whose every key is a field of its kind.
    pub async fn write_item(
        &self,
        id: &str,
        item_type: &str,
        value: &Value,
    ) -> Result<(), SkarbiecError> {
        self.write_described(id, item_type, value, &json!({})).await
    }

    /// Write one item that also carries schema context — the descriptive keys a
    /// kind keeps beside its fields, such as a key pair's fingerprint.
    /// A write always goes to the store, never to `PUT /v1/items`: that route
    /// creates no operator item and replaces a whole one for nobody, so a
    /// direct client had no working write either.
    pub async fn write_described(
        &self,
        id: &str,
        item_type: &str,
        fields: &Value,
        context: &Value,
    ) -> Result<(), SkarbiecError> {
        Box::pin(crate::credential_store::write::write_item_with(
            id, item_type, fields, context,
        ))
        .await
    }

    /// Deletion is an owner act for the same reason a write is.
    pub async fn delete_item(&self, id: &str) -> Result<(), SkarbiecError> {
        Box::pin(crate::credential_store::write::delete_item_with(id)).await
    }

    /// Replace one field of the item named `item` under this client's own
    /// `rotate:<item>#<field>` grant: Skarbiec's `PUT /v1/items` in rotate
    /// mode keeps every other field, the kind, recipients and tags, and records
    /// this consumer as the writer. The item is addressed as named, never
    /// selected by role, like [`Client::read_declared_string`]. Answers the
    /// revision the vault wrote.
    pub async fn rotate_field(
        &self,
        item: &str,
        field: &str,
        value: &str,
    ) -> Result<Option<u64>, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::write::rotate_field_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
                item,
                field,
                value,
            ))
            .await;
        }
        let operation_id = format!("stado-rotate-{}", uuid::Uuid::new_v4());
        let response = self
            .request(reqwest::Method::PUT, "/v1/items")
            .map_err(|error| error.naming(&self.consumer, item, field))?
            .json(&json!({
                "id": item,
                "field": field,
                "mode": "rotate",
                "operation_id": operation_id,
                "value": value,
            }))
            .send()
            .await
            .map_err(|error| SkarbiecError::from(error).naming(&self.consumer, item, field))?;
        let body = Self::response_json(response)
            .await
            .map_err(|error| error.naming(&self.consumer, item, field))?;
        Ok(body.get("revision").and_then(Value::as_u64))
    }
}
