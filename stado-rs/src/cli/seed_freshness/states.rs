//! The vault's half on its own: which login rows hold a usable seed.
//!
//! `authenticator_seed_freshness` joins this with the sign-in journal, which
//! is the right answer for "is the seed still accepted". A caller that is
//! about to CHANGE a seed needs less than that and needs it twice — before
//! and after — so the vault read is named here rather than copied, and the
//! enrolment command in `crate::cli::seed_enrol` reads exactly what the
//! diagnostic reads.

use serde_json::Value;

use crate::cli::seed_freshness::remote::skarbiec::remote_seed_state;
use crate::cli::CmdError;

/// Every login row's seed state on one host, or one row's when named.
///
/// Pairs of `(item, seed_state)` in Skarbiec's own vocabulary: `present`,
/// `declared_empty`, `field_absent`. No seed, code or password is read.
pub async fn seed_states(
    target: &str,
    login_item: Option<&str>,
) -> Result<Vec<(String, String)>, CmdError> {
    let runner = crate::deploy::production_runner();
    let credential_host = crate::cli::host::credential_host(target).await?;
    let mut arguments = vec![String::from("totp-seed-state")];
    if let Some(item) = login_item {
        arguments.push(item.to_string());
    }
    let answer = remote_seed_state(
        &credential_host.target,
        &runner,
        &credential_host.home,
        &credential_host.vault,
        &credential_host.gnupg_home,
        &arguments,
    )
    .await?;
    Ok(rows_of(&answer))
}

/// One item answers as an object, a sweep as `{rows: [...]}`. Both are the
/// same list to a caller. Reachable from the crate because the enrolment
/// command's test reads real Skarbiec answers through it.
pub(crate) fn rows_of(answer: &Value) -> Vec<(String, String)> {
    let rows = match answer.get("rows").and_then(Value::as_array) {
        Some(rows) => rows.clone(),
        None => vec![answer.clone()],
    };
    rows.iter()
        .filter_map(|row| {
            let item = row.get("item").and_then(Value::as_str)?;
            let state = row.get("seed_state").and_then(Value::as_str)?;
            Some((item.to_string(), state.to_string()))
        })
        .collect()
}
