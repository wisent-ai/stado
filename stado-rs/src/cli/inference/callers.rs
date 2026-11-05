//! `stado inference callers add|revoke|list|usage`: who may call a
//! deployment from outside the fleet, and what each called.

use serde_json::json;

use crate::cli::CmdError;
use crate::inference::callers::{self, Caller, Limits, Price};
use crate::queue::JobStorage;

#[derive(clap::Subcommand, Debug)]
pub enum CallersCommands {
    /// Issue a key to one caller of a deployment, with its limits and,
    /// optionally, its price. The key is written to the vault item
    /// `inference-caller-<deployment>-<caller>` (field `token`) and printed
    /// once; the declaration keeps only its SHA-256. Refused when the caller
    /// is already declared.
    Add {
        deployment: String,
        #[arg(long)]
        caller: String,
        #[arg(long = "requests-per-minute")]
        requests_per_minute: u32,
        #[arg(long = "tokens-per-day")]
        tokens_per_day: u64,
        /// US dollars per million prompt tokens; with
        /// --usd-per-million-generated.
        #[arg(
            long = "usd-per-million-prompt",
            requires = "usd_per_million_generated"
        )]
        usd_per_million_prompt: Option<f64>,
        #[arg(
            long = "usd-per-million-generated",
            requires = "usd_per_million_prompt"
        )]
        usd_per_million_generated: Option<f64>,
        #[arg(long)]
        json: bool,
    },
    /// Revoke a caller's key; the gateway refuses it on its next refresh.
    Revoke {
        deployment: String,
        #[arg(long)]
        caller: String,
        #[arg(long)]
        json: bool,
    },
    /// Every caller of a deployment, revoked ones included.
    List {
        deployment: String,
        #[arg(long)]
        json: bool,
    },
    /// Each caller's requests, tokens, refusals and charge per hour.
    Usage {
        deployment: String,
        #[arg(long)]
        caller: Option<String>,
        #[arg(long = "since-hours", default_value_t = 24)]
        since_hours: u32,
        #[arg(long)]
        json: bool,
    },
}

pub(super) async fn known_deployment(name: &str) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = crate::inference::schema::parse(&document).map_err(CmdError::declaration)?;
    if registry
        .deployments
        .iter()
        .any(|deployment| deployment.name == name)
    {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "unknown inference deployment '{name}'; `stado inference list` names them"
    )))
}

pub async fn dispatch(command: CallersCommands) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    match command {
        CallersCommands::Add {
            deployment,
            caller,
            requests_per_minute,
            tokens_per_day,
            usd_per_million_prompt,
            usd_per_million_generated,
            json,
        } => {
            callers::validate_name(&caller).map_err(CmdError::usage)?;
            if requests_per_minute == 0 || tokens_per_day == 0 {
                return Err(CmdError::usage(
                    "--requests-per-minute and --tokens-per-day must be above zero: a caller \
                     allowed nothing has no use for a key",
                ));
            }
            let price = match (usd_per_million_prompt, usd_per_million_generated) {
                (Some(prompt), Some(generated)) => {
                    if !(prompt.is_finite()
                        && generated.is_finite()
                        && prompt >= 0.0
                        && generated >= 0.0)
                    {
                        return Err(CmdError::usage("a price is a number at or above zero"));
                    }
                    Some(Price {
                        usd_per_million_prompt: prompt,
                        usd_per_million_generated: generated,
                    })
                }
                _ => None,
            };
            known_deployment(&deployment).await?;
            if callers::load(&store, &deployment, &caller).await?.is_some() {
                return Err(CmdError::refused(format!(
                    "caller {caller} of {deployment} is already declared; revoke it and add a new \
                     name to issue another key"
                )));
            }
            let key = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let item = callers::vault_item(&deployment, &caller);
            crate::skarbiec::Client::configured()
                .map_err(CmdError::from)?
                .write_item(&item, "stado-secret", &json!({"token": key}))
                .await
                .map_err(CmdError::from)?;
            let declared = Caller {
                caller: caller.clone(),
                deployment: deployment.clone(),
                key_sha256: callers::key_hash(&key),
                limits: Limits {
                    requests_per_minute,
                    tokens_per_day,
                },
                price,
                issued_at: chrono::Utc::now().to_rfc3339(),
                revoked_at: None,
            };
            callers::save(&store, &declared).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"caller": declared, "key": key, "vault_item": item})
                    )?
                );
            } else {
                println!(
                    "issued caller {caller} of {deployment}; the key is shown once and kept in the \
                     vault item {item}:\n{key}"
                );
            }
            Ok(())
        }
        CallersCommands::Revoke {
            deployment,
            caller,
            json,
        } => {
            let Some(mut declared) = callers::load(&store, &deployment, &caller).await? else {
                return Err(CmdError::missing(format!(
                    "caller {caller} of {deployment} is not declared; `stado inference callers \
                     list {deployment}` names them"
                )));
            };
            if declared.revoked_at.is_some() {
                return Err(CmdError::refused(format!(
                    "caller {caller} of {deployment} is already revoked"
                )));
            }
            declared.revoked_at = Some(chrono::Utc::now().to_rfc3339());
            callers::save(&store, &declared).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&declared)?);
            } else {
                println!(
                    "revoked caller {caller} of {deployment}; the gateway refuses its key from its \
                     next refresh"
                );
            }
            Ok(())
        }
        CallersCommands::List { deployment, json } => {
            let declared = callers::list(&store, &deployment).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&declared)?);
                return Ok(());
            }
            if declared.is_empty() {
                println!(
                    "{deployment} has no caller; `stado inference callers add {deployment}` issues one"
                );
            }
            for caller in &declared {
                let price = match &caller.price {
                    Some(price) => format!(
                        "{} / {} USD per M prompt / generated",
                        price.usd_per_million_prompt, price.usd_per_million_generated
                    ),
                    None => "no price declared".to_string(),
                };
                println!(
                    "{}\t{} req/min\t{} tokens/day\t{price}\t{}",
                    caller.caller,
                    caller.limits.requests_per_minute,
                    caller.limits.tokens_per_day,
                    match &caller.revoked_at {
                        Some(at) => format!("revoked {at}"),
                        None => format!("issued {}", caller.issued_at),
                    }
                );
            }
            Ok(())
        }
        CallersCommands::Usage {
            deployment,
            caller,
            since_hours,
            json,
        } => {
            let since = callers::hour_of(
                chrono::Utc::now() - chrono::Duration::hours(i64::from(since_hours)),
            );
            let hours = callers::usage(&store, &deployment, caller.as_deref(), &since).await?;
            let declared = callers::list(&store, &deployment).await?;
            let price_of = |name: &str| {
                declared
                    .iter()
                    .find(|caller| caller.caller == name)
                    .and_then(|caller| caller.price)
            };
            let rows: Vec<serde_json::Value> = hours
                .iter()
                .map(|hour| {
                    json!({
                        "usage": hour,
                        "charge_usd": price_of(&hour.caller).map(|price| hour.charge(&price)),
                    })
                })
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "schema": "stado.inference-usage.v1",
                        "deployment": deployment,
                        "since": since,
                        "hours": rows,
                    }))?
                );
                return Ok(());
            }
            println!("CALLER\tHOUR\tREQUESTS\tPROMPT\tGENERATED\tUNREAD\tREFUSED\tCHARGE");
            for (hour, row) in hours.iter().zip(&rows) {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    hour.caller,
                    hour.hour,
                    hour.requests,
                    hour.prompt_tokens,
                    hour.generated_tokens,
                    hour.usage_unread,
                    hour.refused,
                    match row["charge_usd"].as_f64() {
                        Some(charge) => format!("{charge:.4} USD"),
                        None => "no price declared".to_string(),
                    }
                );
            }
            Ok(())
        }
    }
}
