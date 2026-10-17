//! Bounded Boat API value objects and redacted failures.
//!
//! Port of `stado/providers/box/_types.py`. The frozen Python dataclasses
//! map to plain structs with public fields; the exception trio maps to
//! [`BoxError`] variants, with the structured, redacted API failure kept as
//! a standalone [`BoxApiError`] payload (status/code/message/request_id/
//! retryable) so callers can match on a missing sandbox and on
//! retryability.
//!
//! `constants` holds the API base, the status constants and the sandbox-id
//! pattern, `errors` the error enum with the API failure, `records` the five
//! record families, and `payload` the redaction, coercion and
//! envelope-parsing helpers those records are read with.

mod constants;
mod errors;
mod payload;
mod records;

/// Named out of tree by `http` (`DEFAULT_BASE_URL` and the retryable-status
/// test), by `client::lifecycle` (`HTTP_NOT_FOUND` on delete) and by
/// `client::validate_box_id` (`box_id_pattern`).
pub use constants::{box_id_pattern, transient_status, DEFAULT_BOX_API_URL, HTTP_NOT_FOUND};
/// Named out of tree as `crate::providers::r#box::{BoxError, BoxApiError}`:
/// re-exported again by the parent, converted by `providers::ProviderError`
/// and matched on `api.status == 404` by the `scheduler::dispatch::box`
/// runtime and passes.
pub use errors::{BoxApiError, BoxError};
/// Named out of tree by the `client::commands`, `client::files`,
/// `client::lifecycle` and `client::prompts` field readers and by
/// `http::api_error`.
pub(crate) use payload::{first_truthy_str, jbool, jstr};
/// `parse_box_info` is named out of tree by `client::lifecycle`,
/// `required_dict` by `http::parse_json` and `client::prompts`, `safe_text`
/// by `http::transport_error`, and `safe_text_limited` carries its
/// published `crate::providers::r#box::types` path.
pub use payload::{
    parse_box_info, parse_sandbox_envelope, required_dict, safe_text, safe_text_limited,
};
/// Named out of tree as `crate::providers::r#box::{BoxInfo, BoxLimits,
/// BoxCommandResult, BoxPromptRun, BoxEventPage}`: re-exported again by the
/// parent and returned by the `client` and `adapter` verbs.
pub use records::{
    BoxCommandResult, BoxEventPage, BoxInfo, BoxLimits, BoxPromptRun, BoxState, PromptStatus,
};
