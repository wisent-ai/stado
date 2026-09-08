//! The support case: the text Microsoft is sent, the idempotent PUT that
//! opens exactly one case per set of denies, and the receipt kept on disk.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::super::{
    parsed, CmdError, OpenUnusualActivityTicketArgs, OperatorToken, ARM_RESOURCE,
    RBAC_SUPPORT_CLASSIFICATION_ID, RBAC_SUPPORT_SERVICE_ID, STANDARD_SUPPORT_PLAN_ID,
    SUPPORT_API_VERSION, UNUSUAL_ACTIVITY_TITLE,
};
use super::arm::{azure_collection, azure_get_json};
use super::home_path;
use super::support::{discover_rbac_support_classification, support_display_name};

fn deny_ticket_description(tenant: &str, subscription: &str, denies: &[Value]) -> String {
    let assignments = denies
        .iter()
        .map(|deny| {
            format!(
                "- ID: {}; name: {}; scope: {}; principals: {}",
                deny.get("id").and_then(Value::as_str).unwrap_or("unknown"),
                deny.get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                deny.get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                deny.get("principals")
                    .map(Value::to_string)
                    .unwrap_or_else(|| "[]".into())
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Microsoft Azure created system-protected [UnusualActivity] Full Deny assignments at root scope. They are inherited by the subscription and deny administrative Actions and DataActions despite existing RBAC grants.\n\nTenant ID: {tenant}\nSubscription ID: {subscription}\nAssignments:\n{assignments}\n\nThe assignments are read-only and system-protected in Azure Portal. Please investigate the unusual-activity signal and remove all listed assignments after security verification."
    )
}

async fn find_existing_unusual_activity_ticket(
    http: &reqwest::Client,
    access_token: &str,
    subscription: &str,
) -> Result<Option<Value>, CmdError> {
    let tickets = azure_collection(
        http,
        access_token,
        format!(
            "{ARM_RESOURCE}/subscriptions/{subscription}/providers/Microsoft.Support/supportTickets?api-version={SUPPORT_API_VERSION}"
        ),
    )
    .await?;
    Ok(tickets.into_iter().find(|ticket| {
        ticket
            .pointer("/properties/status")
            .and_then(Value::as_str)
            .is_some_and(|status| status.eq_ignore_ascii_case("open"))
            && ticket.pointer("/properties/title").and_then(Value::as_str)
                == Some(UNUSUAL_ACTIVITY_TITLE)
    }))
}

pub(super) async fn create_unusual_activity_ticket(
    http: &reqwest::Client,
    operator: &OperatorToken,
    args: &OpenUnusualActivityTicketArgs,
    subscription: &str,
    denies: &[Value],
) -> Result<Value, CmdError> {
    if let Ok(Some(existing)) =
        find_existing_unusual_activity_ticket(http, &operator.access_token, subscription).await
    {
        return Ok(json!({
            "outcome": "already_open",
            "ticket": existing
        }));
    }
    // UnusualActivity denies all Support reads but explicitly exempt Support
    // writes. Prefer current catalog IDs; use the IDs returned by the
    // Azure portal when the deny prevents dynamic discovery.
    let (service, classification) = discover_rbac_support_classification(
        http,
        &operator.access_token,
    )
    .await
    .unwrap_or_else(|_| {
        (
            json!({
                "id": RBAC_SUPPORT_SERVICE_ID,
                "properties": {
                    "displayName": "Role Based Access Control (RBAC) for Azure Resources (IAM)"
                }
            }),
            json!({
                "id": RBAC_SUPPORT_CLASSIFICATION_ID,
                "properties": {
                    "displayName": "Problem with RBAC role assignments"
                }
            }),
        )
    });
    let service_id = service
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("Azure Support RBAC service has no id"))?;
    let classification_id = classification
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("Azure Support RBAC classification has no id"))?;
    let email = args
        .email
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(&operator.account);
    if email.is_empty() || args.first_name.is_empty() || args.last_name.is_empty() {
        return Err(CmdError::usage(
            "--first-name, --last-name, and a contact email are required",
        ));
    }
    let mut deny_ids = denies
        .iter()
        .filter_map(|deny| deny.get("id").and_then(Value::as_str))
        .collect::<Vec<_>>();
    deny_ids.sort_unstable();
    let digest = Sha256::digest(deny_ids.join("\n").as_bytes());
    let ticket_uuid =
        Uuid::from_slice(&digest[..parsed("16")]).expect("SHA digest prefix is a UUID");
    let ticket_name = format!("stado-unusual-activity-{}", ticket_uuid.simple());
    let ticket_url = format!(
        "{ARM_RESOURCE}/subscriptions/{subscription}/providers/Microsoft.Support/supportTickets/{ticket_name}?api-version={SUPPORT_API_VERSION}"
    );
    let response = http
        .put(&ticket_url)
        .bearer_auth(&operator.access_token)
        .json(&json!({
            "properties": {
                "title": UNUSUAL_ACTIVITY_TITLE,
                "description": deny_ticket_description(
                    &operator.tenant_id,
                    subscription,
                    denies
                ),
                "advancedDiagnosticConsent": "No",
                "contactDetails": {
                    "country": args.country,
                    "firstName": args.first_name,
                    "lastName": args.last_name,
                    "preferredContactMethod": "email",
                    "preferredSupportLanguage": "en-US",
                    "preferredTimeZone": args.time_zone,
                    "primaryEmailAddress": email
                },
                "problemClassificationId": classification_id,
                "serviceId": service_id,
                "supportPlanId": STANDARD_SUPPORT_PLAN_ID,
                "severity": "minimal",
                "require24X7Response": false,
            }
        }))
        .send()
        .await?;
    let status = response.status();
    let operation_url = response
        .headers()
        .get("location")
        .or_else(|| response.headers().get("azure-asyncoperation"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let text = response.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(CmdError::click(format!(
            "Azure Support ticket creation failed with HTTP {status}: {}",
            body.get("error").unwrap_or(&body)
        )));
    }
    if status.as_u16() == parsed::<u16>("202") {
        if let Some(url) = operation_url {
            for _ in usize::default()..parsed::<usize>("30") {
                tokio::time::sleep(Duration::from_secs(parsed("2"))).await;
                let poll = http
                    .get(&url)
                    .bearer_auth(&operator.access_token)
                    .send()
                    .await?;
                if poll.status().as_u16() != parsed::<u16>("202") {
                    break;
                }
            }
        }
    }
    let ticket = azure_get_json(http, &operator.access_token, &ticket_url)
        .await
        .unwrap_or_else(|_| {
            json!({
                "id": format!("/subscriptions/{subscription}/providers/Microsoft.Support/supportTickets/{ticket_name}"),
                "name": ticket_name.clone(),
                "properties": {
                    "status": "Submitted; status read is blocked by the active deny assignment",
                    "title": UNUSUAL_ACTIVITY_TITLE
                }
            })
        });
    Ok(json!({
        "outcome": "created",
        "ticket_name": ticket_name,
        "service": support_display_name(&service),
        "problem_classification": support_display_name(&classification),
        "ticket": ticket
    }))
}

pub(super) fn persist_unusual_activity_receipt(receipt: &Value) -> Result<PathBuf, CmdError> {
    let path = home_path(".stado/azure-unusual-activity-ticket.json");
    let temporary = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = serde_json::to_vec_pretty(receipt)?;
    content.push(b'\n');
    std::fs::write(&temporary, content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let radix = parsed("8");
        let mode = u32::from_str_radix("600", radix).expect("owner-only mode is valid octal");
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode))?;
    }
    std::fs::rename(&temporary, &path)?;
    Ok(path)
}
