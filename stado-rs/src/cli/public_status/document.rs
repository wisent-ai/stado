//! The published status document, `public-status/components.json` in the
//! fleet's object store: one row per declared component with its state, since
//! when it has been in that state, the last look and the last failed one.
//!
//! Each check folds into the document it finds: a component whose state did
//! not change keeps its `since`, one that changed starts it at this check, and
//! the last failure stays until a newer one replaces it, so "up since 10:02,
//! last failed 09:58 with 502 …" is read from one place.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::queue::JobStorage;

/// Where the document lives in the object store.
pub(super) const BLOB: &str = "public-status/components.json";

/// The word a working component's row carries.
pub(super) const UP: &str = "up";
/// The word a failing component's row carries.
pub(super) const DOWN: &str = "down";

/// One component's look, ready to fold.
pub(super) struct Observed {
    pub(super) product: String,
    pub(super) component: String,
    pub(super) title: String,
    pub(super) up: bool,
    pub(super) detail: String,
}

/// The published document, or `None` when no check has published one.
pub(super) async fn read(store: &JobStorage) -> Result<Option<Value>, CmdError> {
    let Some(text) = store.download_text(BLOB).await? else {
        return Ok(None);
    };
    let document = serde_json::from_str(&text).map_err(|error| {
        CmdError::click(format!(
            "{BLOB} in the object store is not a status document: {error}"
        ))
    })?;
    Ok(Some(document))
}

/// The row `previous` holds for `product`/`component`.
fn before<'a>(previous: Option<&'a Value>, product: &str, component: &str) -> Option<&'a Value> {
    previous
        .and_then(|document| document["components"].as_array())
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["product"] == product && row["component"] == component)
        })
}

/// The document after this check: every observed component, its state
/// carried forward from `previous` where it did not change.
pub(super) fn fold(
    previous: Option<&Value>,
    observed: &[Observed],
    checked_at: &str,
    checked_by: &str,
) -> Value {
    let components: Vec<Value> = observed
        .iter()
        .map(|look| {
            let state = match look.up {
                true => UP,
                false => DOWN,
            };
            let earlier = before(previous, &look.product, &look.component);
            let since = match earlier {
                Some(row) if row["state"] == state => row["since"].clone(),
                _ => json!(checked_at),
            };
            let last_failure = match (look.up, earlier) {
                (false, _) => json!({ "at": checked_at, "error": look.detail }),
                (true, Some(row)) => row["last_failure"].clone(),
                (true, None) => Value::Null,
            };
            json!({
                "product": look.product,
                "component": look.component,
                "title": look.title,
                "state": state,
                "since": since,
                "checked_at": checked_at,
                "detail": look.detail,
                "last_failure": last_failure,
            })
        })
        .collect();
    json!({
        "checked_at": checked_at,
        "checked_by": checked_by,
        "components": components,
    })
}

/// Replace the published document with `document`.
pub(super) async fn publish(store: &JobStorage, document: &Value) -> Result<(), CmdError> {
    store
        .upload_text(BLOB, &serde_json::to_string_pretty(document)?)
        .await?;
    Ok(())
}
