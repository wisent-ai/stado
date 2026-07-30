//! GCP authentication through the adapter host's metadata identity or the
//! adapter's scoped `stado-gcp` Skarbiec item. Static ADC files, gcloud
//! sessions, process-environment credentials, and workload-agent grants are
//! deliberately unsupported provider credential sources.

use serde_json::Value;

use super::{Client, SkarbiecError};

pub async fn gcp_provider() -> Result<std::sync::Arc<dyn gcp_auth::TokenProvider>, SkarbiecError> {
    match gcp_auth::MetadataServiceAccount::new().await {
        Ok(identity) => Ok(std::sync::Arc::new(identity)),
        Err(metadata_error) => {
            let item = Client::configured_item("stado-gcp").await.map_err(|error| {
                SkarbiecError::GcpAuth(format!(
                    "GCP metadata identity is unavailable ({metadata_error}); scoped stado-gcp read failed: {error}"
                ))
            })?;
            let credential_json = item
                .get("service_account_json")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    (item.get("client_email").is_some() && item.get("private_key").is_some())
                        .then(|| serde_json::to_string(&item))
                        .transpose()
                        .ok()
                        .flatten()
                })
                .ok_or_else(|| {
                    SkarbiecError::GcpAuth(
                        "stado-gcp must contain service_account_json or a service-account JSON object"
                            .to_string(),
                    )
                })?;
            let identity =
                gcp_auth::CustomServiceAccount::from_json(&credential_json).map_err(|error| {
                    SkarbiecError::GcpAuth(format!(
                        "stado-gcp service-account JSON is invalid: {error}"
                    ))
                })?;
            Ok(std::sync::Arc::new(identity))
        }
    }
}
