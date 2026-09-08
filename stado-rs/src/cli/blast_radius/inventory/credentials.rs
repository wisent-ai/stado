//! The credential store probe: is the globally selected Skarbiec reachable,
//! and does it still hold the items the fleet requires.

use std::collections::BTreeSet;

use crate::cli::blast_radius::CredentialStoreReport;

pub(in crate::cli::blast_radius) async fn inspect_credential_store() -> CredentialStoreReport {
    const REQUIRED_ITEMS: &[&str] = &["stado-huggingface"];
    let locator = crate::credential_store::requested_selector()
        .unwrap_or_else(|error| format!("invalid selector: {error}"));
    let credentials = match crate::credential_store::admin_credentials() {
        Ok(credentials) => credentials,
        Err(error) => {
            return CredentialStoreReport {
                state: "unreachable".to_string(),
                locator,
                consumer: String::new(),
                item_count: None,
                items: Vec::new(),
                missing_required: REQUIRED_ITEMS
                    .iter()
                    .map(|item| (*item).to_string())
                    .collect(),
                error: Some(error.to_string()),
            }
        }
    };
    let consumer = credentials.consumer.clone();
    let client = match crate::skarbiec::Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    ) {
        Ok(client) => client,
        Err(error) => {
            return CredentialStoreReport {
                state: "unreachable".to_string(),
                locator,
                consumer,
                item_count: None,
                items: Vec::new(),
                missing_required: REQUIRED_ITEMS
                    .iter()
                    .map(|item| (*item).to_string())
                    .collect(),
                error: Some(error.to_string()),
            }
        }
    };
    let listed = match tokio::time::timeout(crate::doctor::PROBE_TIMEOUT, client.list_items()).await
    {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => Err(format!(
            "credential store inspection exceeded {:?}",
            crate::doctor::PROBE_TIMEOUT
        )),
    };
    match listed {
        Ok(mut items) => {
            items.retain(|item| item.deleted != Some(true));
            items.sort_by(|left, right| left.id.cmp(&right.id));
            let present: BTreeSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
            let missing_required: Vec<String> = REQUIRED_ITEMS
                .iter()
                .copied()
                .filter(|item| !present.contains(item))
                .map(str::to_string)
                .collect();
            CredentialStoreReport {
                state: if missing_required.is_empty() {
                    "reachable"
                } else {
                    "degraded"
                }
                .to_string(),
                locator,
                consumer,
                item_count: Some(items.len()),
                items,
                missing_required,
                error: None,
            }
        }
        Err(error) => CredentialStoreReport {
            state: "unreachable".to_string(),
            locator,
            consumer,
            item_count: None,
            items: Vec::new(),
            missing_required: REQUIRED_ITEMS
                .iter()
                .map(|item| (*item).to_string())
                .collect(),
            error: Some(error.to_string()),
        },
    }
}
