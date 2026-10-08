//! GCP authentication through the adapter host's metadata identity or the
//! Skarbiec item that plays the `cloud-gcp` role, read through the adapter's
//! scoped grant. Static ADC files, gcloud sessions, process-environment
//! credentials, and workload-agent grants are deliberately unsupported
//! provider credential sources.

use super::{Client, SkarbiecError};

/// The role whose item holds the service-account key.
const ROLE: &str = "cloud-gcp";
const FIELD: &str = "service_account_json";

pub async fn gcp_provider() -> Result<std::sync::Arc<dyn gcp_auth::TokenProvider>, SkarbiecError> {
    match gcp_auth::MetadataServiceAccount::new().await {
        Ok(identity) => Ok(std::sync::Arc::new(identity)),
        Err(metadata_error) => {
            let credential_json = Client::configured()?
                .read_string(ROLE, FIELD)
                .await
                .map_err(|error| {
                    SkarbiecError::GcpAuth(format!(
                        "GCP metadata identity is unavailable ({metadata_error}); \
                         scoped {ROLE}#{FIELD} read failed: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    SkarbiecError::GcpAuth(format!("the {ROLE} item must contain {FIELD}"))
                })?;
            let identity =
                gcp_auth::CustomServiceAccount::from_json(&credential_json).map_err(|error| {
                    SkarbiecError::GcpAuth(format!(
                        "the {ROLE} service-account JSON is invalid: {error}"
                    ))
                })?;
            Ok(std::sync::Arc::new(identity))
        }
    }
}
