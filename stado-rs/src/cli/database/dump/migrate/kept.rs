//! The copies `migrate` keeps beside a database's item: `<item>-previous`,
//! the item as it stood before the move, and `<item>-fleet-pending`, the
//! fleet coordinates a failed restore leaves for a second run.
//!
//! Skarbiec refuses a second item holding exactly the payload of another
//! (`… already holds exactly this payload; an exact duplicate is refused`),
//! so a copy written unchanged was refused whenever it equalled the item:
//! `<item>-previous` always does when it is written, which stopped every
//! move before anything was placed. A kept copy is not the credential in
//! use, and its context says so: `kept_as` names its role, the item it
//! stands for and when it was kept, which also tells a reader of the vault
//! why the copy is there. The item itself never carries `kept_as`.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// The context field a kept copy carries.
const KEPT_AS: &str = "kept_as";

/// `document` as kept beside `item` in `role` (`previous` or
/// `fleet-pending`); refused when it holds no context object to say so.
pub(super) fn copy(document: &Value, item: &str, role: &str) -> Result<Value, CmdError> {
    let mut kept = document.clone();
    let context = kept
        .get_mut("context")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click(format!(
                "the {role} copy of {item} holds no context object, so the vault could not tell it from {item}"
            ))
            .stating(FailureCode::Config)
        })?;
    context.insert(
        KEPT_AS.to_string(),
        json!({ "role": role, "of": item, "at": chrono::Utc::now().to_rfc3339() }),
    );
    Ok(kept)
}

/// `document` as the item in use: a kept copy put back loses `kept_as`.
pub(super) fn in_use(mut document: Value) -> Value {
    if let Some(context) = document.get_mut("context").and_then(Value::as_object_mut) {
        context.remove(KEPT_AS);
    }
    document
}
