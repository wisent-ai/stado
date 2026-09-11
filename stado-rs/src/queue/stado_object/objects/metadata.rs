//! What the gateway says about an object rather than what is in it: one stat
//! read, and the RFC 3339 timestamp parse every descriptor goes through.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};
use serde::Deserialize;

use crate::queue::StorageError;
use crate::remote::object_store::ObjectRef;

use super::super::{ObjectDescriptor, StadoObjectBackend};

impl StadoObjectBackend {
    pub(super) async fn stat(&self, path: &str) -> Result<Option<ObjectDescriptor>, StorageError> {
        let object = self.object(path)?;
        let mut url = self.url("/api/object/stat");
        url.query_pairs_mut()
            .append_pair("uri", &object.to_string());
        let response = Self::send_through_boundary(self.request(Method::GET, url)).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        #[derive(Deserialize)]
        struct StatResponse {
            uri: String,
            #[serde(default)]
            size: Option<u64>,
            #[serde(default)]
            updated_at: Option<String>,
            #[serde(default)]
            metadata: BTreeMap<String, String>,
        }
        let stat: StatResponse = response.json().await?;
        let key = ObjectRef::parse(&stat.uri)?.key().to_string();
        Ok(Some(ObjectDescriptor {
            key,
            size: stat.size,
            updated_at: stat.updated_at,
            metadata: stat.metadata,
        }))
    }

    pub(super) fn parse_updated(
        value: Option<String>,
    ) -> Result<Option<DateTime<Utc>>, StorageError> {
        value
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(|timestamp| timestamp.with_timezone(&Utc))
                    .map_err(|error| {
                        StorageError::Other(format!(
                            "Stado object API returned invalid updated_at {value:?}: {error}"
                        ))
                    })
            })
            .transpose()
    }
}
