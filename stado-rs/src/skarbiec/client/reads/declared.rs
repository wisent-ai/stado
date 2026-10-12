//! Reads of an item a boundary declaration names: the configuration chose
//! the item, so it is read as named and never looked up as a role, with the
//! version the value was read under.

use serde_json::{json, Value};

use crate::skarbiec::envelope::plain;
use crate::skarbiec::{ItemVersion, SkarbiecError, VersionedValue};

use super::super::Client;

impl Client {
    /// Resolve one optional string field of the item a boundary declaration
    /// names (`object_api.namespaces.*.item`, `release_api.publishers`,
    /// `machine_api.clients`, `service_api.deployers`, `registry_api.clients`):
    /// the configuration chose that item, so it is read as named and never
    /// looked up as a role. Selecting it by role closed every boundary of the
    /// object API with `503 object authorization unavailable`, because no
    /// verifier item plays a role. `None` when the item or field is absent.
    pub async fn read_declared_string(
        &self,
        item: &str,
        field: &str,
    ) -> Result<Option<String>, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::read_declared_string_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
                item,
                field,
            ))
            .await
            .map_err(|error| error.naming(&self.consumer, item, field));
        }
        let response = self
            .send(
                self.request(reqwest::Method::POST, "/v1/items/read")?
                    .json(&json!({"id": item, "field": field})),
            )
            .await
            .map_err(|error| error.naming(&self.consumer, item, field))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let body = Self::response_json(response)
            .await
            .map_err(|error| error.naming(&self.consumer, item, field))?;
        plain(
            body.get("value")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
        .map_err(|error| error.naming(&self.consumer, item, field))
    }

    /// [`Client::read_declared_string`] with the version the value was read
    /// under (`/v1/items/read` answers `item`, `item_uid` and `revision`
    /// beside the value). `None` when the item or field is absent. A vault
    /// older than item revisions answers no version: the value comes back
    /// with none, so the holder reads it again on every check, the way a
    /// store without versions is read, and the vault is named once on stderr
    /// as the one to update. It used to be refused outright, which closed
    /// every object route of the fleet — and with them the release path that
    /// delivers the newer Skarbiec to that host.
    pub async fn read_declared_versioned(
        &self,
        item: &str,
        field: &str,
    ) -> Result<Option<VersionedValue>, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::read_declared_versioned_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
                item,
                field,
            ))
            .await
            .map_err(|error| error.naming(&self.consumer, item, field));
        }
        let Some(body) = self.declared_post("/v1/items/read", item, field).await? else {
            return Ok(None);
        };
        let value = plain(
            body.get("value")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
        .map_err(|error| error.naming(&self.consumer, item, field))?;
        let Some(value) = value else {
            return Ok(None);
        };
        Ok(Some(VersionedValue {
            value,
            version: self.version(&body, item, field),
        }))
    }

    /// The version a read of the declared item's field would answer now,
    /// without the value (`/v1/items/revision`, the same read grant): nothing
    /// is decrypted, so a holder of the value checks it on every request.
    /// `None` when the item or field is absent, and `None` from a vault older
    /// than item revisions (named once on stderr): the holder then reads the
    /// value on every check.
    pub async fn read_declared_revision(
        &self,
        item: &str,
        field: &str,
    ) -> Result<Option<ItemVersion>, SkarbiecError> {
        if self.route_store {
            return Box::pin(crate::credential_store::read_declared_revision_with(
                &self.base_url,
                &self.consumer,
                &self.token_file,
                self.grant_mode,
                item,
                field,
            ))
            .await
            .map_err(|error| error.naming(&self.consumer, item, field));
        }
        let Some(body) = self
            .declared_post("/v1/items/revision", item, field)
            .await?
        else {
            return Ok(None);
        };
        Ok(self.version(&body, item, field))
    }

    /// The version fields of a single-field answer, or `None` from a vault
    /// that answers none: one older than item revisions. Said once per
    /// process on stderr, with the vault and what to update, so the fleet
    /// keeps serving (every holder reads the value per check) while the
    /// upgrade is delivered through the object plane that would otherwise
    /// have been closed by this very answer.
    fn version(&self, body: &Value, item: &str, field: &str) -> Option<ItemVersion> {
        match serde_json::from_value(body.clone()) {
            Ok(version) => Some(version),
            Err(error) => {
                static SAID: std::sync::Once = std::sync::Once::new();
                SAID.call_once(|| {
                    eprintln!(
                        "[skarbiec] {}",
                        SkarbiecError::NoItemVersion(format!(
                            "{item}#{field} at {}: {error}",
                            self.base_url
                        ))
                    );
                });
                None
            }
        }
    }

    /// POST `{id, field}` to `path`; `None` on `404`, the body otherwise.
    async fn declared_post(
        &self,
        path: &str,
        item: &str,
        field: &str,
    ) -> Result<Option<Value>, SkarbiecError> {
        let response = self
            .send(
                self.request(reqwest::Method::POST, path)?
                    .json(&json!({"id": item, "field": field})),
            )
            .await
            .map_err(|error| error.naming(&self.consumer, item, field))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Self::response_json(response)
            .await
            .map(Some)
            .map_err(|error| error.naming(&self.consumer, item, field))
    }
}
