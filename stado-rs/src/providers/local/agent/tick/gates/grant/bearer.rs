//! Binding a declared consumer grant to the bearer file its product reads,
//! when that file is on the host that holds the owner vault: asked of the
//! vault the way a read would be, and re-issued against the file when it does
//! not open the grant or the grant has an end.

use std::path::Path;

use serde_json::Value;

use crate::cli::secrets::launcher_json;

/// What binding a declared grant to the bearer file on this host did.
pub(super) enum Bound {
    /// The file already opened a grant that lives until revoked.
    Already,
    /// The file opened the grant, whose end is gone now.
    Kept,
    /// The file did not open the grant; it is now bound to it.
    Rebound,
}

/// Whether the bearer in `file` opens `consumer`'s grant: asked of the vault
/// with the grant's first capability, the way a read would be.
fn opens(
    launcher: &Path,
    vault: &Path,
    consumer: &str,
    grant: &Value,
    file: &Path,
) -> Result<bool, String> {
    let first = grant
        .get("capabilities")
        .and_then(Value::as_array)
        .and_then(|capabilities| capabilities.first())
        .ok_or_else(|| {
            format!("the owner vault lists {consumer} with no capability to ask about")
        })?;
    let (Some(action), Some(item)) = (
        first.get("action").and_then(Value::as_str),
        first.get("item").and_then(Value::as_str),
    ) else {
        return Err(format!(
            "{consumer}'s first capability names no action or item: {first}"
        ));
    };
    let file = file.to_string_lossy();
    let mut probe: Vec<&str> = vec![
        "grant",
        "verify",
        consumer,
        item,
        "--action",
        action,
        "--token-file",
        file.as_ref(),
    ];
    if let Some(field) = first.get("field").and_then(Value::as_str) {
        probe.extend(["--field", field]);
    }
    let verdict = launcher_json(launcher, vault, &probe).map_err(|error| error.to_string())?;
    verdict
        .get("allowed")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("skarbiec grant verify answered no allowed verdict: {verdict}"))
}

/// Re-issue `consumer`'s grant until revoked against the bearer in `file`,
/// with the capabilities and audience it holds, when that bearer does not
/// open it or it has an end; then ask again, so a bind that did not take is
/// the error. The vault reads the file; no bearer is generated or rotated.
pub(super) fn bind(
    launcher: &Path,
    vault: &Path,
    consumer: &str,
    grant: &Value,
    file: &Path,
    has_end: bool,
) -> Result<Bound, String> {
    let opened = opens(launcher, vault, consumer, grant, file)?;
    if opened && !has_end {
        return Ok(Bound::Already);
    }
    let capabilities = super::vault::grant_record(&Value::Array(vec![grant.clone()]), consumer)?
        .map(|(_, capabilities)| capabilities.join(","))
        .ok_or_else(|| format!("the owner vault's row for {consumer} could not be read back"))?;
    let audience = grant
        .get("audience")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("the owner vault lists {consumer} with no audience: {grant}"))?;
    let file_name = file.to_string_lossy();
    launcher_json(
        launcher,
        vault,
        &[
            "grant",
            "issue",
            consumer,
            "--capabilities",
            capabilities.as_str(),
            "--audience",
            audience,
            "--token-file",
            file_name.as_ref(),
            "--until-revoked",
        ],
    )
    .map_err(|error| format!("re-issuing it against {file_name} was refused: {error}"))?;
    if !opens(launcher, vault, consumer, grant, file)? {
        return Err(format!(
            "it was re-issued against {file_name}, and that bearer still does not open it"
        ));
    }
    Ok(if opened { Bound::Kept } else { Bound::Rebound })
}
