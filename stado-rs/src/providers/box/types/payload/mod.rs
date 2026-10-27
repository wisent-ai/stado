//! The Boat wire-payload layer: redaction, coercion and sandbox parsing.
//!
//! `redaction` holds `safe_text` with its three patterns, `coercion` the
//! field readers together with `required_dict`, and `parse` the sandbox
//! record and envelope readers.

mod coercion;
mod parse;
mod redaction;

/// Named out of tree by `http::parse_json` and by the `client::prompts`
/// `promptRun` readers.
pub use coercion::required_dict;
/// Named out of tree by `http::api_error` and by the `client::commands`,
/// `client::files`, `client::lifecycle` and `client::prompts` readers.
pub(crate) use coercion::{first_stated, jbool, jint, jstr};
/// Named out of tree by the `client::lifecycle` create / get / list /
/// update / fork verbs.
pub use parse::{parse_box_info, parse_sandbox_envelope};
/// `safe_text` is named out of tree by `http::transport_error` and inside
/// this tree by `errors::BoxApiError::new`; `safe_text_limited` carries the
/// published `crate::providers::r#box::types::safe_text_limited` path.
pub use redaction::{safe_text, safe_text_limited};
