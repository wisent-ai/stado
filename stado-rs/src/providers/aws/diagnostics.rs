//! The two places an AWS failure becomes words: the `[aws]` progress line
//! Python's `_log` prints, and the lift that embeds the EC2 error code in
//! [`ProviderError::Aws`] so the substring classification in `provider`
//! ("InsufficientInstanceCapacity", "InvalidInstanceID.NotFound") keeps
//! matching on `error.to_string()`.

use crate::providers::ProviderError;

/// Python `_log`.
pub(super) fn log(msg: &str) {
    eprintln!("[aws] {msg}");
}

/// Lift an [`aws_sdk_ec2::error::SdkError`] into [`ProviderError::Aws`],
/// embedding the service error code so Python's substring classification
/// keeps working on the message.
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
