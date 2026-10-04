//! The credential store probe: is the globally selected Skarbiec reachable,
//! and does an item still play every role the fleet's agents are declared to
//! read (`agent.skarbiec.roles`). Nothing here names an item: the roles come
//! from the configuration and the vault says which item plays each.

use crate::cli::blast_radius::CredentialStoreReport;

/// Every role the configuration declares for the fleet's agents.
fn required_roles() -> Vec<String> {
    crate::config::agent_skarbiec_roles().to_vec()
}

pub(in crate::cli::blast_radius) async fn inspect_credential_store() -> CredentialStoreReport {
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
                missing_required: required_roles(),
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
                missing_required: required_roles(),
                error: Some(error.to_string()),
            }
        }
    };
    let listed = client.list_items().await.map_err(|error| error.to_string());
    match listed {
        Ok(mut items) => {
            items.retain(|item| item.deleted != Some(true));
            items.sort_by(|left, right| left.id.cmp(&right.id));
            let missing_required: Vec<String> = required_roles()
                .into_iter()
                .filter(|role| crate::skarbiec::roles::holders(&items, role).is_empty())
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
            missing_required: required_roles(),
            error: Some(error.to_string()),
        },
    }
}
