//! Bounded Box API value objects and redacted failures.
//!
//! Port of `stado/providers/box/_types.py`. The frozen Python dataclasses
//! map to plain structs with public fields; the exception trio maps to
//! [`BoxError`] variants, with the structured, redacted API failure kept as
//! a standalone [`BoxApiError`] payload (status/code/message/request_id/
//! retryable) so callers can match on 404s and retryability exactly like
//! the Python `except BoxAPIError as exc: exc.status == 404` sites.
//!
//! `constants` holds the endpoint, timeout, bound and status constants with
//! the box-id pattern, `errors` the error enum with the API failure,
//! `records` the five record families, and `payload` the redaction,
//! coercion and envelope-parsing helpers those records are read with.

mod constants;
mod errors;
mod payload;
mod records;

/// Named out of tree by `http` (`DEFAULT_BASE_URL`, `DEFAULT_TIMEOUT`, the
/// bounded read and the retryable-status test), by `client::lifecycle`
/// (`HTTP_NOT_FOUND` on delete) and by `client::validate_box_id`
/// (`box_id_pattern`).
pub use constants::{
    box_id_pattern, DEFAULT_BOX_API_URL, DEFAULT_TIMEOUT_SECONDS, HTTP_NOT_FOUND, MAX_JSON_BYTES,
    TRANSIENT_HTTP,
};
/// Named out of tree as `crate::providers::r#box::{BoxError, BoxApiError}`:
/// re-exported again by the parent, converted by `providers::ProviderError`
/// and matched on `api.status == 404` by the `scheduler::dispatch::box`
/// runtime and passes.
pub use errors::{BoxApiError, BoxError};
/// Named out of tree by the `client::commands`, `client::files`,
/// `client::lifecycle` and `client::prompts` field readers and by
/// `http::api_error`.
pub(crate) use payload::{first_truthy_str, jbool, jint_or, jstr};
/// `parse_box_info` is named out of tree by `client::lifecycle`,
/// `required_dict` by `http::parse_json` and `client::prompts`, `safe_text`
/// by `http::transport_error`, and `safe_text_limited` carries its
/// published `crate::providers::r#box::types` path.
pub use payload::{parse_box_info, required_dict, safe_text, safe_text_limited};
/// Named out of tree as `crate::providers::r#box::{BoxInfo, BoxLimits,
/// BoxCommandResult, BoxPromptRun, BoxEventPage}`: re-exported again by the
/// parent and returned by the `client` and `adapter` verbs.
pub use records::{BoxCommandResult, BoxEventPage, BoxInfo, BoxLimits, BoxPromptRun};
