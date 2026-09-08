use serde_json::Value;

use crate::cli::host::checks::probes::cell;

/// The vault and vault-sidecar tables of [`super::inventory`], returning both
/// sections so the reconciliation half can count them.
pub(super) fn print_vaults(
    report: &Value,
    section: &dyn Fn(&str) -> Vec<Value>,
) -> (Vec<Value>, Vec<Value>) {
    let vaults = section("vaults");
    if vaults.is_empty() {
        println!("\nvaults: none — $HOME/.stado holds no *.vault.json");
    } else {
        crate::cli::table::print(
            &["VAULT", "STATE", "BYTES", "MODE", "OWNER ONLY"],
            &vaults
                .iter()
                .map(|vault| {
                    vec![
                        cell(vault.get("name")),
                        cell(vault.get("state")),
                        cell(vault.get("bytes")),
                        cell(vault.get("mode")),
                        cell(vault.get("owner_only")),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }
    // Snapshots, pre-migration copies and acquisitions files, kept in their
    // own table on purpose: the active vault is state, a sidecar is history,
    // and editing the wrong one is the mistake this separation prevents.
    let sidecars = section("vault_sidecars");
    if !sidecars.is_empty() {
        crate::cli::table::print(
            &["VAULT SIDECAR", "STATE", "BYTES", "MODE", "OWNER ONLY"],
            &sidecars
                .iter()
                .map(|sidecar| {
                    vec![
                        cell(sidecar.get("name")),
                        cell(sidecar.get("state")),
                        cell(sidecar.get("bytes")),
                        cell(sidecar.get("mode")),
                        cell(sidecar.get("owner_only")),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }
    if report.get("vaults_truncated") == Some(&Value::Bool(true))
        || report.get("vault_sidecars_truncated") == Some(&Value::Bool(true))
    {
        println!(
            "$HOME/.stado holds more vault files than this command lists; \
             vaults_seen and vault_sidecars_seen carry the real counts."
        );
    }
    println!(
        "Vault rows are metadata only. This command never opens a vault, so no \
         ciphertext, item id, consumer name or token can appear above."
    );

    (vaults, sidecars)
}
