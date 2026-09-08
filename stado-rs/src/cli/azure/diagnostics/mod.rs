//! `azure unusual-activity` — the read verbs: what Azure's system-protected
//! deny assignments are doing to this subscription, and the one support case
//! that is the only way to get them lifted.

use std::path::PathBuf;

use serde_json::{json, Value};

use super::session::refresh_operator_token;
use super::{CmdError, UnusualActivityArgs, UnusualActivityCommands};

mod arm;
mod denies;
mod support;
mod ticket;

use denies::list_unusual_activity_denies;
use ticket::{create_unusual_activity_ticket, persist_unusual_activity_receipt};

fn home_path(relative: &str) -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(relative)
}

fn configured_subscription(explicit: Option<&str>) -> Result<String, CmdError> {
    let subscription = explicit
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| crate::config::azure_subscription_id().to_string());
    if subscription.is_empty() {
        Err(CmdError::usage(
            "--subscription or AZURE_SUBSCRIPTION_ID is required",
        ))
    } else {
        Ok(subscription)
    }
}

pub(in crate::cli::azure) async fn unusual_activity(
    args: UnusualActivityArgs,
) -> Result<(), CmdError> {
    match args.command {
        UnusualActivityCommands::Diagnose(args) => {
            let subscription = configured_subscription(args.subscription.as_deref())?;
            let operator = refresh_operator_token(&args.operator_item).await?;
            let http = reqwest::Client::new();
            let denies =
                list_unusual_activity_denies(&http, &operator.access_token, &subscription).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "operator": {
                        "account": operator.account,
                        "tenant_id": operator.tenant_id,
                        "credential": args.operator_item
                    },
                    "subscription_id": subscription,
                    "active_unusual_activity_denies": denies.len(),
                    "deny_assignments": denies,
                    "resolution": if denies.is_empty() {
                        "no_active_unusual_activity_deny"
                    } else {
                        "run `stado azure unusual-activity open-ticket ... --confirm`; Azure Support must remove system-protected assignments"
                    }
                }))?
            );
            Ok(())
        }
        UnusualActivityCommands::OpenTicket(args) => {
            if !args.confirm {
                return Err(CmdError::usage(
                    "--confirm is required because this command creates an external Azure Support case",
                ));
            }
            let subscription = configured_subscription(args.common.subscription.as_deref())?;
            let operator = refresh_operator_token(&args.common.operator_item).await?;
            let http = reqwest::Client::new();
            let denies =
                list_unusual_activity_denies(&http, &operator.access_token, &subscription).await?;
            if denies.is_empty() {
                return Err(CmdError::click(
                    "no active system-protected UnusualActivity deny assignment was found",
                ));
            }
            let result =
                create_unusual_activity_ticket(&http, &operator, &args, &subscription, &denies)
                    .await?;
            let mut receipt = json!({
                "saved_at": chrono::Utc::now().to_rfc3339(),
                "subscription_id": subscription,
                "deny_assignment_count": denies.len(),
                "support_request": result
            });
            let receipt_path = home_path(".stado/azure-unusual-activity-ticket.json");
            receipt["receipt_file"] = Value::String(receipt_path.to_string_lossy().into_owned());
            persist_unusual_activity_receipt(&receipt)?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
            Ok(())
        }
    }
}
