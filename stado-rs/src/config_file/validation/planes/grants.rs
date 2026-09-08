//! The two grant sections a document can widen beyond what a job may hold:
//! the workload items and item#field references jobs read, and the messaging
//! consumer whose grant must stay its own file.

use serde_json::{Map, Value};

use crate::config_file::readers::{field_in, get_in};

/// The workload items the document declares, having judged every `item#field`
/// reference against them. Returned because the sections below ask whether an
/// infrastructure or verifier item leaked into that same list.
pub(in crate::config_file::validation) fn workload_secret_fields(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) -> Vec<Value> {
    let configured_items = field_in(root, &crate::capabilities::AGENT_SKARBIEC_ITEMS_CONFIG)
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
                        "agent.skarbiec.secret_fields entries must be item#field strings"
                            .to_string(),
                    );
                    continue;
                };
                let Some((item, field)) = reference.split_once('#') else {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields entry {reference:?} must be item#field"
                    ));
                    continue;
                };
                if item.is_empty()
                    || field.is_empty()
                    || reference.matches('#').count() != std::iter::once(()).count()
                {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields entry {reference:?} must contain one non-empty item#field"
                    ));
                }
                if !configured_items
                    .iter()
                    .any(|configured| configured.as_str() == Some(item))
                {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields entry {reference:?} names an item absent from agent.skarbiec.items"
                    ));
                }
                if matches!(
                    item,
                    "stado-aws"
                        | "stado-azure"
                        | "stado-gcp"
                        | "stado-machine-api"
                        | "stado-service-api"
                        | "stado-host-health-api"
                ) || item.ends_with("-object-api")
                    || item.ends_with("-release-publisher")
                {
                    problems.push(format!(
                        "agent.skarbiec.secret_fields must not expose infrastructure item {item:?} to jobs"
                    ));
                }
            }
        }
        Some(_) => problems.push(
            "agent.skarbiec.secret_fields must be an array of item#field strings".to_string(),
        ),
    }
    configured_items
}

/// The business-messaging grant: its own consumer, its own owner-only token
/// file, and exactly the item set the notifier reads.
pub(in crate::config_file::validation) fn messaging(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) {
    // Section-presence gate rather than a key read: the checks below apply only
    // to a messaging section an operator chose to declare at all.
    let messaging = get_in(root, "backend.messaging.skarbiec").and_then(Value::as_object);
    if messaging.is_some() {
        let messaging_consumer = field_in(
            root,
            &crate::capabilities::BACKEND_MESSAGING_SKARBIEC.consumer,
        )
        .and_then(Value::as_str)
        .unwrap_or_default();
        if messaging_consumer != "wisent-backend-business-messaging" {
            problems.push(
            "backend.messaging.skarbiec.consumer must be the dedicated wisent-backend-business-messaging consumer"
                .to_string(),
        );
        }
        let messaging_token_file = field_in(
            root,
            &crate::capabilities::BACKEND_MESSAGING_SKARBIEC.token_file,
        )
        .and_then(Value::as_str)
        .unwrap_or_default();
        if messaging_token_file.is_empty() {
            problems.push(
            "backend.messaging.skarbiec.token_file must name the owner-only messaging grant file"
                .to_string(),
        );
        }
        for other in [
            crate::capabilities::SECRETS_SKARBIEC,
            crate::capabilities::AGENT_SKARBIEC,
            crate::capabilities::OBJECT_API_SKARBIEC,
            crate::capabilities::RELEASE_API_SKARBIEC,
            crate::capabilities::SERVICE_API_SKARBIEC,
        ] {
            if !messaging_token_file.is_empty()
                && field_in(root, &other.token_file).and_then(Value::as_str)
                    == Some(messaging_token_file)
            {
                problems.push(format!(
                    "backend.messaging.skarbiec.token_file must be distinct from {}",
                    other.token_file.path
                ));
            }
        }
        let required_messaging_items = [
            "wisent-backend-apns",
            "wisent-backend-fcm",
            "stado-supabase",
        ];
        let optional_email_item = "wisent-backend-email-provider";
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
            problems.push(
            "backend.messaging.skarbiec.items must contain exactly wisent-backend-apns, wisent-backend-fcm, and stado-supabase; wisent-backend-email-provider is optional"
                .to_string(),
        );
        }
        if messaging.is_some_and(|section| section.contains_key("token")) {
            problems.push(
            "backend.messaging.skarbiec.token is forbidden; store the grant only in its owner-only token_file"
                .to_string(),
        );
        }
    }
}
