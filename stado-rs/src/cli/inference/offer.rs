//! `stado inference offer set|show|withdraw|settlements`: one call of a
//! deployment sold to a caller with no key
//! ([`crate::inference::pay_per_call`]).

use serde_json::json;

use crate::cli::CmdError;
use crate::inference::callers::Price;
use crate::inference::pay_per_call::{self, Method, Offer};
use crate::queue::JobStorage;

#[derive(clap::Subcommand, Debug)]
pub enum OfferCommands {
    /// Declare or replace the offer: the price, the most tokens one call may
    /// take, how long a quote holds, and each payment method as
    /// NAME=VERIFY_URL[,VAULT_ITEM]. The gateway reads it on its next
    /// refresh. Refused without a method, a price, or a bound above zero.
    Set {
        deployment: String,
        #[arg(long = "usd-per-million-prompt")]
        usd_per_million_prompt: f64,
        #[arg(long = "usd-per-million-generated")]
        usd_per_million_generated: f64,
        #[arg(long = "max-prompt-tokens")]
        max_prompt_tokens: u64,
        #[arg(long = "max-generated-tokens")]
        max_generated_tokens: u64,
        #[arg(long = "quote-seconds")]
        quote_seconds: u64,
        /// A payment method: NAME=VERIFY_URL, or NAME=VERIFY_URL,VAULT_ITEM
        /// when the endpoint needs the item's `token` as its bearer;
        /// repeatable.
        #[arg(long = "method", required = true)]
        methods: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// The deployment's offer, and the most one call costs under it.
    Show {
        deployment: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop selling calls; keys already paid for still make their call.
    Withdraw {
        deployment: String,
        #[arg(long)]
        json: bool,
    },
    /// Each paid call: the method, what was paid, the outcome, the tokens,
    /// the cost and the refund due.
    Settlements {
        deployment: String,
        #[arg(long = "since-hours", default_value_t = 24)]
        since_hours: u32,
        #[arg(long)]
        json: bool,
    },
}

/// One `--method NAME=URL[,ITEM]`.
fn method(text: &str) -> Result<Method, CmdError> {
    let (name, rest) = text.split_once('=').ok_or_else(|| {
        CmdError::usage(format!(
            "--method {text}: expected NAME=VERIFY_URL[,VAULT_ITEM]"
        ))
    })?;
    crate::inference::callers::validate_name(name).map_err(|_| {
        CmdError::usage(format!(
            "--method {text}: the method name {name:?} must be lowercase letters, digits and inner \
             hyphens"
        ))
    })?;
    let (url, item) = match rest.split_once(',') {
        Some((url, item)) if !item.is_empty() => (url, Some(item.to_string())),
        Some(_) => {
            return Err(CmdError::usage(format!(
                "--method {text}: the vault item after the comma is empty"
            )))
        }
        None => (rest, None),
    };
    let parsed = url::Url::parse(url).map_err(|error| {
        CmdError::usage(format!("--method {text}: {url} is not a URL: {error}"))
    })?;
    if parsed.scheme() != "https" && parsed.host_str() != Some("127.0.0.1") {
        return Err(CmdError::usage(format!(
            "--method {text}: a payment is verified over https, or on this host's loopback \
             127.0.0.1"
        )));
    }
    Ok(Method {
        name: name.to_string(),
        verify_url: url.to_string(),
        credential_item: item,
    })
}

fn price_line(offer: &Offer) -> String {
    format!(
        "{} / {} USD per M prompt / generated; at most {} prompt and {} generated tokens a call, \
         so one call costs at most {:.6} USD; a quote holds {} s; methods {}",
        offer.price.usd_per_million_prompt,
        offer.price.usd_per_million_generated,
        offer.max_prompt_tokens,
        offer.max_generated_tokens,
        offer.bound_usd(),
        offer.quote_seconds,
        offer
            .methods
            .iter()
            .map(|method| method.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub async fn dispatch(command: OfferCommands) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    match command {
        OfferCommands::Set {
            deployment,
            usd_per_million_prompt,
            usd_per_million_generated,
            max_prompt_tokens,
            max_generated_tokens,
            quote_seconds,
            methods,
            json,
        } => {
            if !(usd_per_million_prompt.is_finite()
                && usd_per_million_generated.is_finite()
                && usd_per_million_prompt >= 0.0
                && usd_per_million_generated >= 0.0)
            {
                return Err(CmdError::usage("a price is a number at or above zero"));
            }
            if max_prompt_tokens == 0 || max_generated_tokens == 0 || quote_seconds == 0 {
                return Err(CmdError::usage(
                    "--max-prompt-tokens, --max-generated-tokens and --quote-seconds must be above \
                     zero: a call allowed nothing, or a quote nobody can pay, sells nothing",
                ));
            }
            let methods = methods
                .iter()
                .map(|text| method(text))
                .collect::<Result<Vec<_>, _>>()?;
            let mut names: Vec<&str> = methods.iter().map(|method| method.name.as_str()).collect();
            names.sort_unstable();
            if names.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(CmdError::usage("each --method name is given once"));
            }
            super::callers::known_deployment(&deployment).await?;
            let offer = Offer {
                deployment: deployment.clone(),
                price: Price {
                    usd_per_million_prompt,
                    usd_per_million_generated,
                },
                max_prompt_tokens,
                max_generated_tokens,
                quote_seconds,
                methods,
                offered_at: chrono::Utc::now().to_rfc3339(),
            };
            pay_per_call::save_offer(&store, &offer).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&offer)?);
            } else {
                println!(
                    "{deployment} sells one call without a key: {}; the gateway reads it on its \
                     next refresh",
                    price_line(&offer)
                );
            }
            Ok(())
        }
        OfferCommands::Show { deployment, json } => {
            let offer = pay_per_call::offer(&store, &deployment).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "offer": offer,
                        "bound_usd": offer.as_ref().map(Offer::bound_usd),
                    }))?
                );
                return Ok(());
            }
            match offer {
                Some(offer) => println!("{deployment}: {}", price_line(&offer)),
                None => println!(
                    "{deployment} sells no call without a key; `stado inference offer set \
                     {deployment}` declares an offer"
                ),
            }
            Ok(())
        }
        OfferCommands::Withdraw { deployment, json } => {
            if !pay_per_call::withdraw_offer(&store, &deployment).await? {
                return Err(CmdError::missing(format!(
                    "{deployment} has no pay-per-call offer to withdraw"
                )));
            }
            if json {
                println!("{}", json!({"deployment": deployment, "withdrawn": true}));
            } else {
                println!(
                    "{deployment} sells no call without a key from the gateway's next refresh; \
                     keys already paid for still make their one call"
                );
            }
            Ok(())
        }
        OfferCommands::Settlements {
            deployment,
            since_hours,
            json,
        } => {
            let since =
                (chrono::Utc::now() - chrono::Duration::hours(i64::from(since_hours))).to_rfc3339();
            let kept = pay_per_call::settlements(&store, &deployment, &since).await?;
            let earned: f64 = kept.iter().filter_map(|row| row.earned_usd()).sum();
            let refunds: f64 = kept.iter().filter_map(|row| row.refund_due_usd).sum();
            let unread = kept
                .iter()
                .filter(|row| row.refund_due_usd.is_none())
                .count();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "schema": "stado.inference-pay-per-call-settlements.v1",
                        "deployment": deployment,
                        "since": since,
                        "settlements": kept,
                        "earned_usd": earned,
                        "refunds_due_usd": refunds,
                        "cost_unread": unread,
                    }))?
                );
                return Ok(());
            }
            println!("SETTLED\tQUOTE\tMETHOD\tPAID\tOUTCOME\tPROMPT\tGENERATED\tCOST\tREFUND DUE");
            for row in &kept {
                let tokens = |count: Option<u64>| {
                    count.map_or("unread".to_string(), |count| count.to_string())
                };
                let usd = |amount: Option<f64>| {
                    amount.map_or("unread".to_string(), |amount| format!("{amount:.6}"))
                };
                println!(
                    "{}\t{}\t{}\t{:.6}\t{}\t{}\t{}\t{}\t{}",
                    row.settled_at,
                    row.quote,
                    row.method,
                    row.usd_paid,
                    row.outcome,
                    tokens(row.prompt_tokens),
                    tokens(row.generated_tokens),
                    usd(row.cost_usd),
                    usd(row.refund_due_usd),
                );
            }
            println!(
                "earned {earned:.6} USD; refunds due {refunds:.6} USD{}",
                match unread {
                    0 => String::new(),
                    n => format!("; {n} call(s) stated no usage, so their cost is unread"),
                }
            );
            Ok(())
        }
    }
}
