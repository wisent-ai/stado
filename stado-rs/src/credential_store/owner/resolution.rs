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
/// by a reader holding only a path from ANOTHER machine — `stado host vaults`
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
/// The discovery order is Skarbiec's own — `$HOME/.local/share/skarbiec`,
/// then `$HOME/.stado`, then `$HOME`, the list `skarbiec`'s `vaults` command
/// searches. Stado used to name `$HOME/.stado/skarbiec.vault.json` alone,
/// while the `skarbiec` CLI defaults to `.local/share/skarbiec`. Two tools on
/// one machine, two answers, and no way for an operator to see the
/// disagreement: on 2026-09-05 six `skarbiec set-json` writes went to
/// `.local/share/skarbiec` and were simultaneously real, `active` on the
/// host, and invisible to `stado repair stado --step release-verifier`, which read
/// the other file. That closed the fleet's release publication boundary for
/// every product until the declarations were retracted.
///
/// When two candidates carry the SAME owner identity the machine has no
/// single authoritative vault, and picking either silently is exactly the
/// failure above. That is refused, naming both paths and their item counts,
/// because an operator who is told can declare the one they mean and a
/// program that guesses cannot be corrected. The contents are never merged
/// here: which items belong where is the operator's decision.
///
/// The answer is read from `secrets.skarbiec.vault_file`, so it is one
/// declaration this and every later command shares —
/// `SKARBIEC_VAULT_FILE` still overrides it, which is how a build is
/// exercised before it is installed, but an environment variable answers for
/// one process and the split brain outlives it.
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
    let present: Vec<(PathBuf, String, usize)> = candidates
        .iter()
        .filter(|path| path.is_file())
        .filter_map(|path| vault_identity(path).map(|(owner, items)| (path.clone(), owner, items)))
        .collect();
    if present.len() > usize::from(true) {
        let first_owner = &present[usize::default()].1;
        if present.iter().all(|(_, owner, _)| owner == first_owner) {
            let described = present
                .iter()
                .map(|(path, _, items)| format!("{} ({items} items)", path.display()))
                .collect::<Vec<_>>()
                .join(" and ");
            return Err(SkarbiecError::Deployment(format!(
                "this machine holds {} vaults that all claim owner {first_owner}: {described}. \
                 There is no single authoritative vault, so a credential write or an \
                 authoritative read here would silently pick one — which is how six real items \
                 became invisible to the release verifier. Declare the one you mean, which \
                 every later command then shares: `stado config set \
                 secrets.skarbiec.vault_file <path>` locally, or `stado host config-set \
                 <target> secrets.skarbiec.vault_file <path>` for a managed host. \
                 `stado credentials vault` reports this state and each candidate's owner and \
                 item count, and `stado host vaults <target>` reports the same for a managed \
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
         that holds the vault (`stado host vaults` names them)",
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
    Ok(VAULT_CANDIDATE_TAILS
        .iter()
        .map(|tail| PathBuf::from(format!("{home}{tail}")))
        .filter(|path| path.is_file())
        .filter_map(|path| {
            vault_identity(&path).map(|(owner, items)| {
                json!({
                    "path": path.display().to_string(),
                    "owner": owner,
                    "items": items,
                })
            })
        })
        .collect())
}

/// One candidate's owner identity and item count, or `None` when it is not a
/// vault at all — a backup or a half-written file is simply not a candidate.
fn vault_identity(path: &std::path::Path) -> Option<(String, usize)> {
    let document: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?)
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
