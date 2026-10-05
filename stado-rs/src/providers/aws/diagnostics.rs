//! AWS progress and operation diagnostics. Native absence handling uses the
//! SDK's service code before converting other failures to a diagnostic.

use crate::providers::ProviderError;

/// Emit the provider's progress message.
pub(super) fn log(msg: &str) {
    eprintln!("[aws] {msg}");
}

/// Retain the failed EC2 operation, native service code and service message.
pub(super) fn ec2_error<E>(desc: &str, err: &aws_sdk_ec2::error::SdkError<E>) -> ProviderError
where
    E: aws_sdk_ec2::error::ProvideErrorMetadata + std::fmt::Debug,
{
    if let Some(service) = err.as_service_error() {
        let code = service.code().unwrap_or("");
        let message = service.message().unwrap_or("");
        return ProviderError::Aws(format!("EC2 {desc} failed: {code}: {message}"));
    }
    ProviderError::Aws(format!("EC2 {desc} failed: {err}"))
}
