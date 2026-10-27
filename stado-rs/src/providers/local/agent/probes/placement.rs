//! The worker's placement policy, asserted from the host's registry entry.

use std::path::PathBuf;

use serde_json::Value;

use crate::targets::ComputeTarget;

/// Put this host's registry-declared Weles policy into the file its worker
/// reads, and report `(detail, (enabled, actions))`.
///
/// The document is built by [`crate::cli::placement::policy_document`] — the
/// same builder `stado route placement publish` uses, so the bytes an
/// operator delivers from the coordinator and the bytes this writes are one
/// shape decided in one place.
///
/// Three properties the operator path also holds:
///
///   stamped     the generation is read with the document, and a read that
///               cannot produce one writes nothing. An unstamped policy is the
///               untraceable file this whole path exists to retire.
///   atomic      written to a temporary file in the destination directory and
///               renamed, so a worker reading concurrently sees the whole old
///               document or the whole new one.
///   quiet       a file whose entry already carries the declared `enabled` and
///               actions is left alone. `_source.published_at` moves on every
///               build, so comparing whole documents would rewrite the file
///               every pass and hand the worker a fresh mtime for no change.
pub(crate) async fn reconcile_placement_policy(
    target: &ComputeTarget,
) -> Result<(String, (bool, Vec<String>)), String> {
    let (_, generation) = crate::cli::registry::fetch_versioned_document()
        .await
        .map_err(|error| format!("registry generation unavailable: {error}"))?;
    let policy = crate::cli::placement::policy_document(
        target,
        &generation,
        crate::cli::placement::RECONCILED_BY,
    )
    .map_err(|error| error.to_string())?;
    let desired = crate::cli::placement::policy_effect(&policy);

    // The check `apply_policy` performs remotely, performed here for the same
    // reason: a policy whose entries name no identity of this machine does not
    // fail loudly in the worker. Its loader resolves to `enabled: false` and it
    // declines every row in silence — 29,616 times, the last time this fleet
    // learned it — so a writer that cannot see itself in the document must
    // write nothing and say why.
    let identity =
        crate::cli::placement::normalize_hostname(&crate::providers::vast::system_hostname());
    if !names_this_host(&policy, &identity) {
        return Err(format!(
            "the registry declares no identity matching {identity:?} for target {}, so the \
             policy built from it would name every host except this one and the worker would \
             decline every routed row. Declare {identity:?} in that target's `hostnames`",
            target.name
        ));
    }

    let path = placement_policy_path()?;
    // The policy the worker holds now. A file that cannot be read or parsed is
    // replaced like an absent one, but the report says what was replaced, so
    // a corrupted policy is not passed off as a first write.
    let replaced = match std::fs::read_to_string(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(format!("replaced unreadable {}: {error}; ", path.display())),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Err(error) => Some(format!(
                "replaced {}, which was not JSON: {error}; ",
                path.display()
            )),
            Ok(held) if crate::cli::placement::policy_effect(&held) == desired => {
                return Ok((
                    format!("{} already carries the declaration", path.display()),
                    desired,
                ));
            }
            Ok(_) => None,
        },
    };

    let bytes = format!(
        "{}\n",
        serde_json::to_string_pretty(&policy).map_err(|error| error.to_string())?
    );
    // The Weles worker reads this file, so it keeps the mode a shared record
    // has, written whole and flushed with its directory.
    crate::primitives::document::persist_with_mode(
        &path,
        bytes.as_bytes(),
        crate::primitives::file_mode::shared_read(),
    )
    .map_err(|error| format!("cannot install {}: {error}", path.display()))?;
    Ok((
        format!(
            "{}wrote {} at registry generation {generation}",
            replaced.as_deref().map_or("", |note| note),
            path.display()
        ),
        desired,
    ))
}

/// Whether `policy` carries an entry this machine's worker will match, by the
/// loader's own rule: `hostname` equal, or `identity` present in `aliases`,
/// both normalized (`placement-policy.ts`).
fn names_this_host(policy: &Value, identity: &str) -> bool {
    policy
        .get("hosts")
        .and_then(Value::as_array)
        .is_some_and(|hosts| {
            hosts.iter().any(|host| {
                let named = host
                    .get("hostname")
                    .and_then(Value::as_str)
                    .is_some_and(|name| {
                        crate::cli::placement::normalize_hostname(name) == identity
                    });
                named
                    || host
                        .get("aliases")
                        .and_then(Value::as_array)
                        .is_some_and(|aliases| {
                            aliases.iter().filter_map(Value::as_str).any(|alias| {
                                crate::cli::placement::normalize_hostname(alias) == identity
                            })
                        })
            })
        })
}

/// Basename of the worker's policy file, per `placement-policy.ts`.
const PLACEMENT_POLICY_FILE: &str = "placement-policy.json";

/// Where the worker reads it: `WELES_PLACEMENT_POLICY_FILE` when set, else
/// `$HOME/.config/weles/placement-policy.json`. The override is honoured
/// because a writer that ignores it writes a file nobody reads.
fn placement_policy_path() -> Result<PathBuf, String> {
    if let Some(override_path) = std::env::var_os("WELES_PLACEMENT_POLICY_FILE") {
        let path = PathBuf::from(override_path);
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    let home = crate::config_file::home()
        .map_err(|cause| format!("the worker's policy path is unknown: {cause}"))?;
    Ok(home
        .join(".config")
        .join("weles")
        .join(PLACEMENT_POLICY_FILE))
}
