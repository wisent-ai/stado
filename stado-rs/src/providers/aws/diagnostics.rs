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
        // A service error without its code or message is said as such, with
        // the error the SDK holds, not as an empty code and message.
        return ProviderError::Aws(match (service.code(), service.message()) {
            (Some(code), Some(message)) => format!("EC2 {desc} failed: {code}: {message}"),
            (Some(code), None) => format!("EC2 {desc} failed: {code} (no message): {service:?}"),
            (None, Some(message)) => format!("EC2 {desc} failed (no code): {message}"),
            (None, None) => format!("EC2 {desc} failed with no code and no message: {service:?}"),
        });
    }
    ProviderError::Aws(format!("EC2 {desc} failed: {err}"))
}
