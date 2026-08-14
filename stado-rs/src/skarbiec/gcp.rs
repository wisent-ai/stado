//! GCP authentication through the adapter host's metadata identity or the
//! adapter's scoped `stado-gcp` Skarbiec item. Static ADC files, gcloud
//! sessions, process-environment credentials, and workload-agent grants are
//! deliberately unsupported provider credential sources.

use super::{Client, SkarbiecError};

pub async fn gcp_provider() -> Result<std::sync::Arc<dyn gcp_auth::TokenProvider>, SkarbiecError> {
    match gcp_auth::MetadataServiceAccount::new().await {
        Ok(identity) => Ok(std::sync::Arc::new(identity)),
        Err(metadata_error) => {
            let credential_json = Client::configured()?
                .read_string("stado-gcp", "service_account_json")
                .await
                .map_err(|error| {
                    SkarbiecError::GcpAuth(format!(
                        "GCP metadata identity is unavailable ({metadata_error}); \
                         scoped stado-gcp#service_account_json read failed: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    SkarbiecError::GcpAuth(
                        "stado-gcp must contain service_account_json".to_string(),
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
