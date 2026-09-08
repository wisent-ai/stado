//! The secret deliveries: one Skarbiec field into one variable of one
//! managed unit, into its owner-only token file, or into its consumer grant
//! — and the check that the bearer the unit holds actually works.

use super::*;

pub(crate) mod auth_check;
pub(crate) mod grant;
pub(crate) mod sync;

pub(crate) async fn service_secret(item: &str, field: &str) -> Result<String, CmdError> {
    let vault = crate::skarbiec::Client::service_verifier()
        .map_err(|err| CmdError::click(err.to_string()))?;
    // Both callers -- auth-check and secret-sync -- want exactly one field, and
    // asking for the whole item is refused outright by a broker that requires a
    // named field. Ask for what is wanted.
    let stored = vault
        .read_field(item, field)
        .await
        .map_err(|err| CmdError::click(err.to_string()))?;
    stored
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::click(format!(
                "Skarbiec item {item:?} has no non-empty string field {field:?}"
            ))
        })
}
