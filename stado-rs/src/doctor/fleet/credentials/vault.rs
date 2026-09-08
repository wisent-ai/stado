//! Which owner vault answers for this machine, and whether a bare `skarbiec`
//! would open the same one.

use serde_json::Value;

use crate::doctor::Check;

// ---------------------------------------------------------------------------
// 6b. The owner vault this machine writes through
// ---------------------------------------------------------------------------

pub(in crate::doctor) const OWNER_VAULT_ID: &str = "credential-vault";
pub(in crate::doctor) const OWNER_VAULT_TITLE: &str = "One owner vault answers for this machine";
pub(in crate::doctor) const OWNER_VAULT_REMEDY: &str =
    "run `stado credentials vault` to see every candidate with its owner and item count, then \
     `stado config set secrets.skarbiec.vault_file <path>` to name the one this machine means; \
     put the same path in `SKARBIEC_VAULT_FILE` where a bare `skarbiec` is run, so a write and \
     an authoritative read cannot land in different files. Nothing is merged for you";

/// Two vaults claiming one owner is not a curiosity, it is a write going
/// somewhere no reader looks.
///
/// On 2026-09-05 six `skarbiec set-json` writes landed in
/// `~/.local/share/skarbiec/skarbiec.vault.json` — real, `active` on the host,
/// and invisible to `stado repair stado --step release-verifier`, which reads
/// `~/.stado/skarbiec.vault.json`. The fleet's release publication boundary
/// closed for every product and the cause took a day to name, because nothing
/// reported the split: `stado host vaults` answered "8 vault(s)" and said
/// nothing about which one answers.
///
/// So this check asks the resolution question and then one more: whether the
/// answer is also what a bare `skarbiec` on this machine would open. A
/// declaration fixes every Stado path, and Stado passes it to every `skarbiec`
/// it invokes itself — but an operator's own `skarbiec set-json` still follows
/// Skarbiec's discovery order, and where the two disagree the next write is
/// invisible again.
///
/// Metadata only: owner identity, item counts, paths. No item name and no
/// value is read.
pub(in crate::doctor) fn check_owner_vault() -> Check {
    let candidates = match crate::credential_store::owner::candidates_present() {
        Ok(candidates) => candidates,
        Err(error) => {
            return Check::fail(
                OWNER_VAULT_ID,
                OWNER_VAULT_TITLE,
                format!("this machine's vault candidates cannot be read: {error}"),
                OWNER_VAULT_REMEDY,
            )
        }
    };
    let resolved = match crate::credential_store::owner::vault() {
        Ok(path) => path,
        Err(error) => {
            return Check::fail(
                OWNER_VAULT_ID,
                OWNER_VAULT_TITLE,
                error.to_string(),
                OWNER_VAULT_REMEDY,
            )
        }
    };
    let resolved = resolved.display().to_string();
    let rivals: Vec<String> = candidates
        .iter()
        .filter_map(|vault| {
            let path = vault.get("path").and_then(Value::as_str)?;
            (path != resolved).then(|| {
                format!(
                    "{path} ({} items)",
                    vault
                        .get("items")
                        .and_then(Value::as_u64)
                        .unwrap_or_default()
                )
            })
        })
        .collect();
    if rivals.is_empty() {
        return Check::pass(
            OWNER_VAULT_ID,
            OWNER_VAULT_TITLE,
            format!("{resolved} is the only owner vault this machine holds"),
            OWNER_VAULT_REMEDY,
        );
    }
    // `candidates_present` lists this machine's vaults in Skarbiec's own
    // discovery order, so its first entry is what `skarbiec set-json` opens
    // with no `SKARBIEC_VAULT_FILE`. Read as a fact about the files, not asked
    // of the binary: a check that shells out to learn where a write would go
    // has already made the write's mistake when the binary is missing.
    let cli_default = candidates
        .first()
        .and_then(|vault| vault.get("path").and_then(Value::as_str))
        .map(str::to_string);
    match cli_default {
        Some(cli_default) if cli_default != resolved => Check::fail(
            OWNER_VAULT_ID,
            OWNER_VAULT_TITLE,
            format!(
                "Stado resolves {resolved}, and a bare `skarbiec` on this machine opens \
                 {cli_default} instead, so any `skarbiec set-json` run without \
                 SKARBIEC_VAULT_FILE writes an item Stado never reads. Other candidates: {}",
                rivals.join(", ")
            ),
            OWNER_VAULT_REMEDY,
        ),
        _ => Check::pass(
            OWNER_VAULT_ID,
            OWNER_VAULT_TITLE,
            format!(
                "{resolved} answers for this machine and is what a bare `skarbiec` opens; \
                 beside it: {}",
                rivals.join(", ")
            ),
            OWNER_VAULT_REMEDY,
        ),
    }
}
