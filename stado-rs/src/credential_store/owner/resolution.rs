//! Which owner vault this machine resolves to, and what its candidates are.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::skarbiec::SkarbiecError;

use super::discovery::home;

/// Vaults the fleet's operator items may live in when nothing declares one,
/// in the order `skarbiec`'s own `vaults` command searches them, so Stado and
/// Skarbiec cannot answer "which vault" differently.
///
/// Stated as tails rather than whole paths because the same rule is applied
/// by a reader holding only a path from ANOTHER machine — `stado credentials vault list`
/// judges a fleet report whose `$HOME` is not this process's, and a second
/// hand-written list there is how the two would drift.
pub const VAULT_CANDIDATE_TAILS: &[&str] = &[
    "/.local/share/skarbiec/skarbiec.vault.json",
    "/.stado/skarbiec.vault.json",
    "/skarbiec.vault.json",
];

/// Resolve the owner vault this process writes through.
///
/// A machine that does not hold the vault cannot own a credential write, and
/// saying so is the whole point: the alternative is a write that appears to
/// succeed against a store no owner here can open.
///
/// Discovery uses the same candidate paths as Skarbiec so owner reads and
/// writes do not silently address different files.
///
/// Same-owner candidates are compared by item ID and revision. Exactly one
/// candidate containing every other candidate's items at equal or later
/// revisions is selected and reported on stderr. Otherwise the refusal names
/// each candidate and its item count. No vault contents are merged.
///
/// `secrets.skarbiec.vault_file` declares the shared path for subsequent
/// commands; `SKARBIEC_VAULT_FILE` overrides it for the current process.
pub fn vault() -> Result<PathBuf, SkarbiecError> {
    let declared = crate::config::skarbiec_vault_file();
    if !declared.trim().is_empty() {
        let path = PathBuf::from(declared.trim());
        if !path.is_file() {
            return Err(SkarbiecError::Deployment(format!(
                "the declared owner vault {} is not a file on this machine; \
                 correct secrets.skarbiec.vault_file, or clear it to discover one",
                path.display()
            )));
        }
        return Ok(path);
    }
    let home = home()?;
    let candidates: Vec<PathBuf> = VAULT_CANDIDATE_TAILS
        .iter()
        .map(|tail| PathBuf::from(format!("{}{tail}", home.as_str())))
        .collect::<Vec<_>>();
    let mut present: Vec<(PathBuf, String, usize)> = Vec::new();
    for path in candidates.iter().filter(|path| path.is_file()) {
        if let Some((owner, items)) = vault_identity(path)? {
            present.push((path.clone(), owner, items));
        }
    }
    if let [(_, first_owner, _), _, ..] = present.as_slice() {
        if present.iter().all(|(_, owner, _)| owner == first_owner) {
            // One file that already holds every item of every other one, each
            // at the same or a later revision, loses nothing by being chosen:
            // the others are older copies of it. Only that case is decided
            // here; any item another file holds newer, or holds alone, still
            // refuses, because choosing would hide it.
            if let Some(path) = subsuming_vault(&present)? {
                eprintln!(
                    "owner vault: {} holds every item of the other vaults of owner {first_owner} \
                     at the same or a later revision; using it",
                    path.display()
                );
                return Ok(path);
            }
            let described = present
                .iter()
                .map(|(path, _, items)| format!("{} ({items} items)", path.display()))
                .collect::<Vec<_>>()
                .join(" and ");
            return Err(SkarbiecError::Deployment(format!(
                "this machine holds {} vaults that all claim owner {first_owner}: {described}. \
                 There is no single authoritative vault, so a credential write or an \
                 authoritative read here could hide items held only by another candidate. \
                 Declare the one you mean, which every later command then shares: \
                 `stado config set \
                 secrets.skarbiec.vault_file <path>` locally, or `stado host config set \
                 <target> secrets.skarbiec.vault_file <path>` for a managed host. \
                 `stado credentials vault show` reports this state and each candidate's owner and \
                 item count, and `stado credentials vault list --host <target>` reports the same for a managed \
                 host. Nothing is merged for you.",
                present.len()
            )));
        }
    }
    if let Some((path, _, _)) = present.into_iter().next() {
        return Ok(path);
    }
    Err(SkarbiecError::Deployment(format!(
        "no owner vault in {}; this machine cannot write credential items. Declare one with \
         `stado config set secrets.skarbiec.vault_file <path>`, or run the write on the host \
         that holds the vault (`stado credentials vault list` names them)",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// This machine's own candidates, in discovery order, as the same shape a
/// fleet report carries: one rule reads both.
pub fn candidates_present() -> Result<Vec<Value>, SkarbiecError> {
    let home = home()?;
    let mut present = Vec::new();
    for path in VAULT_CANDIDATE_TAILS
        .iter()
        .map(|tail| PathBuf::from(format!("{home}{tail}")))
        .filter(|path| path.is_file())
    {
        if let Some((owner, items)) = vault_identity(&path)? {
            present.push(json!({
                "path": path.display().to_string(),
                "owner": owner,
                "items": items,
            }));
        }
    }
    Ok(present)
}

/// One candidate's owner identity and item count, or `None` when it is not a
/// vault at all — a backup or a half-written file is simply not a candidate.
/// A candidate that cannot be read is an error naming it: skipped, the vault
/// it may be was passed over and another candidate chosen as the owner's.
fn vault_identity(path: &std::path::Path) -> Result<Option<(String, usize)>, SkarbiecError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        SkarbiecError::Deployment(format!(
            "cannot read the vault candidate {}: {error}; no owner vault is chosen while one \
             candidate cannot be read",
            path.display()
        ))
    })?;
    Ok(vault_identity_of(&text))
}

fn vault_identity_of(text: &str) -> Option<(String, usize)> {
    let document: serde_json::Value = serde_json::from_str(text)
        .ok()
        .filter(serde_json::Value::is_object)?;
    let owner = document
        .get("owner")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            document
                .get("management")
                .and_then(|management| management.get("controller"))
                .and_then(serde_json::Value::as_str)
        })?
        .to_string();
    let items = document
        .get("items")
        .and_then(serde_json::Value::as_object)
        .map(serde_json::Map::len)
        .unwrap_or_default();
    Some((owner, items))
}

/// Every item id of one vault file and its revision; `None` when an item
/// carries no revision to compare. A file that cannot be read or parsed is
/// refused naming it: it was the same `None`, and the ambiguity refusal that
/// followed never said that one candidate could not be compared at all.
fn item_revisions(
    path: &std::path::Path,
) -> Result<Option<std::collections::BTreeMap<String, u64>>, SkarbiecError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        SkarbiecError::Deployment(format!(
            "cannot read the vault {} to compare its items with the other candidates: {error}",
            path.display()
        ))
    })?;
    let document: Value = serde_json::from_str(&text).map_err(|error| {
        SkarbiecError::Deployment(format!(
            "the vault {} is not valid JSON, so its items cannot be compared: {error}",
            path.display()
        ))
    })?;
    let Some(items) = document.get("items").and_then(Value::as_object) else {
        return Ok(None);
    };
    Ok(items
        .iter()
        .map(|(id, entry)| Some((id.clone(), entry.get("revision")?.as_u64()?)))
        .collect())
}

/// The one candidate that holds every item of every other candidate at the
/// same or a later revision, if exactly one does.
fn subsuming_vault(present: &[(PathBuf, String, usize)]) -> Result<Option<PathBuf>, SkarbiecError> {
    let mut revisions: Vec<(PathBuf, std::collections::BTreeMap<String, u64>)> = Vec::new();
    for (path, _, _) in present {
        let Some(items) = item_revisions(path)? else {
            return Ok(None);
        };
        revisions.push((path.clone(), items));
    }
    let subsuming: Vec<&PathBuf> = revisions
        .iter()
        .filter(|(path, items)| {
            revisions
                .iter()
                .filter(|(other, _)| other != path)
                .all(|(_, others)| {
                    others
                        .iter()
                        .all(|(id, revision)| items.get(id).is_some_and(|held| held >= revision))
                })
        })
        .map(|(path, _)| path)
        .collect();
    Ok(match subsuming.as_slice() {
        [only] => Some((*only).clone()),
        _ => None,
    })
}
