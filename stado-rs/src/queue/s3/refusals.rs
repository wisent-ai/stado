//! The failed SDK calls, and the one shape they are all lifted into.
//!
//! Nothing in this backend inspects a status twice: a route matches the
//! statuses that are part of its contract (404 as absent, 412 as a lost
//! precondition race) and hands every other failure here, where the S3 error
//! code and the raw HTTP status it came with become one [`StorageError`].

use aws_sdk_s3::error::ProvideErrorMetadata;

use crate::queue::StorageError;

/// (error code, raw HTTP status) of a failed SDK call, when it was a
/// service (S3-side) error.
fn code_and_status<E: ProvideErrorMetadata>(
    err: &aws_sdk_s3::error::SdkError<E>,
) -> (Option<String>, Option<u16>) {
    match err {
        aws_sdk_s3::error::SdkError::ServiceError(se) => (
            se.err().meta().code().map(str::to_string),
            Some(se.raw().status().as_u16()),
        ),
        _ => (None, None),
    }
}

/// Python's `Code in {"404", "NoSuchKey"}` — both arrive as HTTP 404 (we
/// also accept an explicit NoSuchKey/NotFound code for robustness).
pub(super) fn is_not_found<E: ProvideErrorMetadata>(err: &aws_sdk_s3::error::SdkError<E>) -> bool {
    let (code, status) = code_and_status(err);
    status == Some(404) || matches!(code.as_deref(), Some("404" | "NoSuchKey" | "NotFound"))
}

/// Python's `Code in {"PreconditionFailed", "412"}` / raw 412.
pub(super) fn is_precondition_failed<E: ProvideErrorMetadata>(
    err: &aws_sdk_s3::error::SdkError<E>,
) -> bool {
    let (code, status) = code_and_status(err);
    status == Some(412) || matches!(code.as_deref(), Some("PreconditionFailed" | "412"))
}

/// Lift an SDK error into [`StorageError::Other`], embedding the S3 error
/// code so operators see "NoSuchKey" / "AccessDenied" etc.
pub(super) fn sdk_err<E: ProvideErrorMetadata + std::fmt::Debug>(
    op: &str,
    err: aws_sdk_s3::error::SdkError<E>,
) -> StorageError {
    let (code, status) = code_and_status(&err);
    let detail = match (code, status) {
        (Some(code), Some(status)) => format!("{code} (HTTP {status})"),
        (Some(code), None) => code,
        (None, Some(status)) => format!("HTTP {status}"),
        (None, None) => format!("{err}"),
    };
    StorageError::Other(format!("S3 {op} -> {detail}"))
}
