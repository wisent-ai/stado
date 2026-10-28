//! Installing a registry that names a new authority: this host's copy, the
//! store, and after a takeover every other host's copy, with a receipt kept
//! in the placement history either way.

use serde_json::{json, Value};

use super::{config, hand, HANDED_VERSION, PROMOTED_VERSION};
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::targets;

/// What [`install`] did: the receipt, and why it did not land everywhere.
pub(crate) struct Installed {
    pub receipt: Value,
    pub failure: Option<CmdError>,
}

/// How a registry naming a new authority is installed: `to_host` takes the
/// role from `old`; `unanswered` says why it was taken without `old`, and
/// then every other host's copy is handed it; `store_at` names the object
/// API to write the store in, instead of the store this host reads.
pub(crate) struct Handover<'a> {
    pub to_host: &'a str,
    pub old: &'a str,
    pub unanswered: Option<String>,
    pub store_at: Option<&'a str>,
}

/// Write `content`, a registry naming the new authority, into this host's
/// copy, the store, and the other hosts' copies as `handover` says. The
/// receipt is kept in the placement history either way.
pub(crate) async fn install(content: &str, handover: Handover<'_>) -> Result<Installed, CmdError> {
    let Handover {
        to_host,
        old,
        unanswered,
        store_at,
    } = handover;
    let taken_over = unanswered.is_some();
    let document: Value = serde_json::from_str(content)?;
    let generation = document
        .pointer("/service_directory/generation")
        .and_then(Value::as_u64)
        .ok_or_else(|| config("the promoted registry names no directory generation".to_string()))?;
    let version = if taken_over {
        PROMOTED_VERSION
    } else {
        HANDED_VERSION
    };
    targets::store_last_good(content, version).map_err(|refusal| {
        config(format!(
            "this host's last-known-good copy refused the promoted registry: {refusal}"
        ))
    })?;
    // The snapshot kept of the old authority answers for no read any more.
    crate::cli::resolver::forget_recalled_registry();
    let store = hand::stored(store_at, content).await;
    let registry =
        targets::load_registry_from_str(content).map_err(|error| config(error.to_string()))?;
    // After a takeover every other host is handed the registry at once: each
    // is its own channel. After a hand-over the old authority serves it to
    // every resolver, and a copy would refuse it while that authority answers.
    let others: Vec<_> = registry
        .targets
        .iter()
        .filter(|target| taken_over && target.name != to_host && target.name != old)
        .collect();
    let answers =
        futures::future::join_all(others.iter().map(|target| hand::handed(target, content))).await;
    let mut hosts = Vec::new();
    let mut not_handed = Vec::new();
    for (target, answer) in others.iter().zip(answers) {
        match answer {
            Ok(receipt) => hosts.push(json!({"host": target.name, "receipt": receipt})),
            Err(error) => {
                not_handed.push(target.name.clone());
                hosts.push(json!({"host": target.name, "error": error}));
            }
        }
    }
    let complete = store.is_ok() && not_handed.is_empty();
    let receipt = json!({
        "state": if complete { "taken" } else { "incomplete" },
        "how": if taken_over { "taken-over" } else { "handed-over" },
        "authority": to_host,
        "fenced": old,
        "directory_generation": generation,
        "authority_unanswered": unanswered,
        "store": match &store {
            Ok(version) => json!({"version": version}),
            Err(error) => json!({"error": error}),
        },
        "hosts": hosts,
    });
    crate::cli::placement::record_placement("authority", Ok(&receipt));
    let failure = (!complete).then(|| {
        let store_said = match &store {
            Ok(version) => format!("the store was written ({version})"),
            Err(error) => format!("the store was not written ({error})"),
        };
        CmdError::click(format!(
            "the authority role moved to {to_host}, but not everywhere: {store_said}; hosts not handed the registry: [{}]",
            not_handed.join(", ")
        ))
        .stating(FailureCode::InfraDown)
    });
    Ok(Installed { receipt, failure })
}
