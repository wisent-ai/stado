//! Whether a path is there, told apart from whether it can be examined, and
//! whether a document's list is there, told apart from a value that is not
//! a list.
//!
//! `Path::exists` answers false for a path it cannot stat (a permission or
//! I/O fault), so a job's output directory read through it would be taken as
//! absent and its artifacts never uploaded. Only a path that does not
//! exist is absent here; every other cause is an error of the same kind that
//! names the path. `as_array().into_iter().flatten()` answers an empty list
//! for a field that holds a text or an object, so an answer in the wrong
//! shape read as "nothing"; `listed` refuses it naming the field.

use std::io;
use std::path::Path;

use serde_json::Value;

/// True when `path` exists, false when it does not; a path that cannot be
/// examined is an error naming the path and the cause.
pub fn present(path: &Path) -> io::Result<bool> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{} cannot be examined: {error}", path.display()),
        )),
    }
}

/// The entries of the list `value`, which `what` names for a refusal: a
/// list's items, and none where nothing is recorded (a null or a missing
/// field). Any other value is refused naming `what` and the value it holds.
pub fn listed<'a>(value: &'a Value, what: &str) -> Result<&'a [Value], String> {
    match value {
        Value::Array(items) => Ok(items),
        Value::Null => Ok(&[]),
        other => Err(format!("{what} is not a list: it holds {other}")),
    }
}
