//! The target-local mirror of the control-plane vault, and the sync that
//! keeps it honest.

pub(in crate::cli::host) mod read;
pub(in crate::cli::host) mod sync;

use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::secrets::vault::mirror::read::remote_skarbiec_json_at;

pub(super) fn skarbiec_tool_path(home: &str) -> String {
    format!(
        "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/local/MacGPG2/bin:{home}/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    )
}

/// Run one Skarbiec command on TARGET and parse its JSON answer.
///
/// The vault and GnuPG paths are resolved by the target itself. Arguments stay
/// separate all the way through the host channel, so neither bearer material
/// nor an operator-supplied capability enters a remote shell command.
pub(super) async fn remote_skarbiec_json(
    target: &str,
    arguments: &[String],
) -> Result<(ComputeTarget, Value), CmdError> {
    remote_skarbiec_json_at(target, arguments, None, None, None).await
}

/// The mirror `skarbiec sync-pull` replaces the live vault from, relative to
/// the target account's home.
///
/// `sync_dir()` in Skarbiec's own `net::sync` reads `SKARBIEC_SYNC_DIR` and
/// otherwise takes `$HOME/.skarbiec-sync`, and the file inside it is always
/// `vault.enc.json`. Naming it here is what lets the preview list the mirror's
/// items with the same read-only `list` the live vault answers.
const SKARBIEC_MIRROR_RELATIVE: &str = ".skarbiec-sync/vault.enc.json";

/// One item as the target's own `skarbiec list` reports it, reduced to what a
/// sync verdict turns on.
struct MirrorItem {
    revision: i64,
    updated_at: String,
    deleted: bool,
}

fn mirror_items(
    report: &Value,
) -> Result<std::collections::BTreeMap<String, MirrorItem>, CmdError> {
    let rows = report
        .as_array()
        .ok_or_else(|| CmdError::click("Skarbiec list did not answer an array of items"))?;
    let mut items = std::collections::BTreeMap::new();
    for row in rows {
        let Some(id) = row.get("id").and_then(Value::as_str) else {
            continue;
        };
        items.insert(
            id.to_string(),
            MirrorItem {
                revision: row.get("revision").and_then(Value::as_i64).unwrap_or(-1),
                updated_at: row
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                deleted: row.get("deleted").and_then(Value::as_bool) == Some(true),
            },
        );
    }
    Ok(items)
}
