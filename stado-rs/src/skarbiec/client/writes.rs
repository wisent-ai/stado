//! The owner acts: writing an item and deleting one. Both go to the store,
//! never to `PUT /v1/items`.

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
    /// A write always goes to the store, never to `PUT /v1/items`: that route is
    /// the Weles acquisition path and refuses an operator item outright, so a
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
}
