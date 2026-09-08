//! Judging which vault a managed host's own operations resolve to, from what
//! that host reports.

use serde_json::{json, Value};

/// Which of a host's vaults its own credential operations resolve to, given
/// what that host declares in `secrets.skarbiec.vault_file`.
///
/// The counts were never the question an operator arrives with. `stado host
/// vaults lukasz-macbook` answered "8 vault(s)" for months while two of them
/// claimed one owner, and nothing in the report said that every owner write
/// and every authoritative read on that machine was refused because of it —
/// that surfaced only when `stado repair stado --step release-verifier` failed,
/// with the fleet's release publication boundary already closed.
///
/// The three states are the resolution rule itself, and no item name is
/// consulted to decide them: a declared path that is one of the host's own
/// vaults, one candidate to discover, or several candidates under one owner,
/// which is a refusal on that host until it declares which.
pub fn authority(declared: Option<&str>, vaults: &[Value]) -> Value {
    let path_of = |vault: &Value| {
        vault
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    // A host whose installed release predates this key answers with no field
    // at all, which is not the same fact as a host that declares nothing —
    // reading the first as the second is how a reader older than a
    // declaration reports a state the host is not in.
    let Some(declared) = declared else {
        return json!({
            "state": "unreadable",
            "path": Value::Null,
            "detail": "this host's stado release has no secrets.skarbiec.vault_file field, \
                       so what it resolves cannot be read from here",
        });
    };
    let declared = declared.trim();
    if !declared.is_empty() {
        let held = vaults.iter().any(|vault| path_of(vault) == declared);
        return json!({
            "state": if held { "declared" } else { "declared-absent" },
            "path": declared,
            "detail": if held {
                "declared in secrets.skarbiec.vault_file".to_string()
            } else {
                format!("secrets.skarbiec.vault_file names {declared}, which this host does not hold")
            },
        });
    }
    // Only the paths discovery actually searches can answer it. A host holds
    // vaults discovery never looks at — a Weles worker's own store, a
    // migration broker's, an operator's personal one — and counting those as
    // rivals would report a refusal that the host does not make.
    let candidates = vaults
        .iter()
        .filter(|vault| {
            crate::credential_store::owner::VAULT_CANDIDATE_TAILS
                .iter()
                .any(|tail| path_of(vault).ends_with(tail))
        })
        .collect::<Vec<_>>();
    let mut owners = std::collections::BTreeMap::<String, Vec<String>>::new();
    for vault in &candidates {
        let owner = vault
            .get("owner")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if owner.is_empty() {
            continue;
        }
        owners.entry(owner).or_default().push(path_of(vault));
    }
    // Only a same-owner collision is ambiguous: two candidates with two
    // owners are two machines' worth of items in one home, not a question
    // about which one is this host's.
    let contested = owners
        .iter()
        .filter(|(_, paths)| paths.len() > usize::from(true))
        .map(|(owner, paths)| format!("{owner}: {}", paths.join(", ")))
        .collect::<Vec<_>>();
    if !contested.is_empty() {
        return json!({
            "state": "ambiguous",
            "path": Value::Null,
            "detail": format!(
                "several vaults claim one owner ({}), so every owner write and \
                 authoritative read on this host is refused until \
                 secrets.skarbiec.vault_file names one",
                contested.join("; ")
            ),
        });
    }
    match candidates.first() {
        Some(vault) => json!({
            "state": "discovered",
            "path": path_of(vault),
            "detail": "the only candidate this host holds",
        }),
        None => json!({
            "state": "none",
            "path": Value::Null,
            "detail": "this host holds no vault discovery searches, so it cannot write credential items",
        }),
    }
}
