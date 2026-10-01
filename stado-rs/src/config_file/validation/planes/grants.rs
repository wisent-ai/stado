//! The role lists a document grants: the workload roles and role#field
//! references jobs read, and the backend-messaging roles Stado reads as
//! itself.

use serde_json::{Map, Value};

use crate::config_file::readers::{field_in, get_in};

/// The workload roles the document declares, having judged every `role#field`
/// reference against them. Returned because the sections below ask whether an
/// infrastructure or verifier role leaked into that same list.
pub(in crate::config_file::validation) fn workload_secret_fields(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) -> Vec<Value> {
    let configured_roles = field_in(root, &crate::capabilities::AGENT_SKARBIEC_ROLES_CONFIG)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match field_in(
        root,
        &crate::capabilities::AGENT_SKARBIEC_SECRET_FIELDS_CONFIG,
    ) {
        None => {}
        Some(Value::Array(fields)) => {
            for entry in fields {
                let Some(reference) = entry.as_str() else {
                    problems.push(
                        "agent.skarbiec.secret_fields entries must be role#field strings"
                            .to_string(),
                    );
                    continue;
                };
                let Some((role, field)) = reference.split_once('#') else {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields entry {reference:?} must be role#field"
                    ));
                    continue;
                };
                if role.is_empty()
                    || field.is_empty()
                    || reference.matches('#').count() != std::iter::once(()).count()
                {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields entry {reference:?} must contain one non-empty role#field"
                    ));
                }
                if !configured_roles
                    .iter()
                    .any(|configured| configured.as_str() == Some(role))
                {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields entry {reference:?} names a role absent from agent.skarbiec.roles"
                    ));
                }
                if matches!(
                    role,
                    "cloud-aws"
                        | "cloud-azure"
                        | "cloud-gcp"
                        | "machine-api"
                        | "service-api"
                        | "host-health-api"
                ) || role.ends_with("-object-api")
                    || role.ends_with("-release-publisher")
                {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields must not expose infrastructure role {role:?} to jobs"
                    ));
                }
            }
        }
        Some(_) => problems.push(
            "agent.skarbiec.secret_fields must be an array of role#field strings".to_string(),
        ),
    }
    configured_roles
}

/// Backend-messaging roles must match the set the notifier reads.
/// Stado reads them through its own Skarbiec identity.
pub(in crate::config_file::validation) fn messaging(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) {
    // Section-presence gate rather than a key read: the checks below apply only
    // to a messaging section an operator chose to declare at all.
    let messaging = get_in(root, "backend.messaging.skarbiec").and_then(Value::as_object);
    if messaging.is_some() {
        let required_messaging_items = crate::dashboard::operator_auth::REQUIRED_ROLES;
        let optional_email_item = crate::dashboard::operator_auth::EMAIL_ROLE;
        let messaging_items = field_in(
            root,
            &crate::capabilities::BACKEND_MESSAGING_SKARBIEC_ITEMS_CONFIG,
        )
        .and_then(Value::as_array);
        if !messaging_items.is_some_and(|items| {
            required_messaging_items
                .iter()
                .all(|expected| items.iter().any(|item| item.as_str() == Some(expected)))
                && items.iter().all(|item| {
                    item.as_str().is_some_and(|item| {
                        required_messaging_items.contains(&item) || item == optional_email_item
                    })
                })
                && items.iter().enumerate().all(|(index, item)| {
                    items
                        .iter()
                        .skip(index.saturating_add(usize::from(true)))
                        .all(|later| later != item)
                })
        }) {
            problems.push(format!(
                "backend.messaging.skarbiec.items must contain exactly the roles {}; {optional_email_item} is optional",
                required_messaging_items.join(", ")
            ));
        }
    }
}
