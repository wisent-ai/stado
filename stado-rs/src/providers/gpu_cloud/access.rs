//! What a vendor adapter reads before it calls its API: the credential from
//! Skarbiec by role, and its settings from the selected profile.
//!
//! Both refuse by name. A missing credential names the role, the tag that
//! selects it, the field and the command that stores it; a missing setting
//! names its configuration path and environment override. Nothing here falls
//! back to another source.

use crate::capabilities::{GpuCloudVendor, RuntimeFacet};

use super::api::GpuCloudError;

/// One field of the vendor's credential item, the one live item tagged
/// `stado:role:cloud-<provider>`.
pub async fn secret(vendor: GpuCloudVendor, field: &str) -> Result<String, GpuCloudError> {
    let role = vendor.credential_role();
    match crate::skarbiec::read_string(&role, field).await {
        Ok(Some(value)) if !value.trim().is_empty() => Ok(value.trim().to_string()),
        Ok(_) => Err(GpuCloudError::Credential(format!(
            "{}: no live Skarbiec item tagged {} carries a non-empty field {field}; store the \
             {} credential with `stado credentials item put --host <vault host> --role {role} \
             --type api-key` and the JSON object on standard input",
            vendor.display_name(),
            crate::skarbiec::roles::role_tag(&role),
            vendor.display_name(),
        ))),
        Err(error) => Err(GpuCloudError::Credential(format!(
            "{}: reading field {field} of Skarbiec role {role}: {error}",
            vendor.display_name()
        ))),
    }
}

/// One scalar setting of the vendor's configuration section; empty when the
/// profile and the environment carry nothing. A key the catalog does not
/// declare for the vendor is refused rather than read.
pub fn setting(vendor: GpuCloudVendor, key: &str) -> Result<String, GpuCloudError> {
    let provider = vendor.provider().as_str();
    if crate::capabilities::config_field(RuntimeFacet::Compute, provider, key).is_none() {
        return Err(GpuCloudError::Configuration(format!(
            "{}: the adapter reads setting {key:?}, which the capability catalog does not \
             declare for provider {provider}",
            vendor.display_name()
        )));
    }
    Ok(crate::config::gpu_cloud_setting(vendor.provider(), key)
        .trim()
        .to_string())
}

/// A setting the adapter cannot act without.
pub fn required_setting(vendor: GpuCloudVendor, key: &str) -> Result<String, GpuCloudError> {
    let value = setting(vendor, key)?;
    if !value.is_empty() {
        return Ok(value);
    }
    let provider = vendor.provider().as_str();
    let field = crate::capabilities::config_field(RuntimeFacet::Compute, provider, key)
        .ok_or_else(|| {
            GpuCloudError::Configuration(format!(
                "{}: setting {key:?} is not declared for provider {provider}",
                vendor.display_name()
            ))
        })?;
    Err(GpuCloudError::Configuration(format!(
        "{}: {} is not set; set it with `stado config set {} <value>` or the {} environment \
         variable",
        vendor.display_name(),
        field.path,
        field.path,
        field.env
    )))
}
