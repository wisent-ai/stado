//! The service directory generation an answer of `connect` was decided
//! under, stated in the answer and in a refusal's details.

use serde_json::{json, Value};

use crate::cli::directory::document::DIRECTORY_KEY;
use crate::cli::CmdError;

/// The service directory generation `document` states, when it states one.
pub(super) fn generation_of(document: &Value) -> Option<u64> {
    document
        .get(DIRECTORY_KEY)
        .and_then(|block| block.get("generation"))
        .and_then(Value::as_u64)
}

/// `error` with the directory generation it was decided under added to its
/// `--json` details, beside whatever it already found. An error that states
/// its own `generation` keeps it: one this host's own statements decided
/// states `null`, since no directory generation follows a change of them.
pub(super) fn under(mut error: CmdError, generation: Option<u64>) -> CmdError {
    let Some(generation) = generation else {
        return error;
    };
    let details = match error.details.take().map(|details| *details) {
        Some(Value::Object(found)) if found.contains_key("generation") => Value::Object(found),
        Some(Value::Object(mut found)) => {
            found.insert("generation".to_string(), json!(generation));
            Value::Object(found)
        }
        Some(found) => json!({ "generation": generation, "found": found }),
        None => json!({ "generation": generation }),
    };
    error.details = Some(Box::new(details));
    error
}

/// `error` stated as decided by this host's own statements, which no
/// directory generation follows.
pub(super) fn local(error: CmdError) -> CmdError {
    error.detailing(json!({ "generation": null, "decided_by": "this host's own statement" }))
}
