//! `stado market payout record|list`: payouts a source made in a token, kept
//! in the fleet store with the rate each is valued at.
//!
//! A source whose adapter reads its payouts records them the same way; this
//! is the command for a payout the operator reads from the source's own
//! statement. Each is kept at `market/payouts/<source>/<host>/<paid_at>.json`.

use serde_json::json;

use crate::cli::CmdError;
use crate::providers::payout::{TokenPayout, TokenRate};

const PREFIX: &str = "market/payouts";

fn key(payout: &TokenPayout) -> String {
    format!(
        "{PREFIX}/{}/{}/{}.json",
        payout.source, payout.host, payout.paid_at
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn record(
    source: &str,
    host: &str,
    paid_at: &str,
    token: &str,
    amount: f64,
    usd_per_unit: Option<f64>,
    as_of: Option<&str>,
    rate_source: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    if !(amount.is_finite() && amount > 0.0) {
        return Err(CmdError::usage(format!(
            "--amount {amount} is not a payout"
        )));
    }
    chrono::DateTime::parse_from_rfc3339(paid_at).map_err(|error| {
        CmdError::usage(format!(
            "--paid-at {paid_at:?} is not an RFC 3339 time: {error}"
        ))
    })?;
    let rate = match (usd_per_unit, as_of, rate_source) {
        (None, None, None) => None,
        (Some(usd_per_unit), Some(as_of), Some(rate_source)) => Some(TokenRate {
            token: token.to_string(),
            usd_per_unit,
            as_of: as_of.to_string(),
            source: rate_source.to_string(),
        }),
        _ => {
            return Err(CmdError::usage(
                "a rate is --usd-per-unit, --as-of and --rate-source together, or none of them",
            ))
        }
    };
    let payout = TokenPayout {
        source: source.to_string(),
        host: host.to_string(),
        paid_at: paid_at.to_string(),
        token: token.to_string(),
        amount,
        rate,
    };
    let store = crate::queue::JobStorage::new().await?;
    store
        .upload_bytes(&key(&payout), serde_json::to_vec(&payout)?.as_slice())
        .await
        .map_err(|error| CmdError::from(error).within("keep the payout"))?;
    let usd = payout.usd();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "payout": payout,
                "usd": usd.as_ref().ok(),
                "usd_unread": usd.as_ref().err(),
            }))?
        );
    } else {
        match usd {
            Ok(usd) => {
                println!("kept {amount} {token} from {source} on {host} at {paid_at}: {usd:.2} USD")
            }
            Err(cause) => println!(
                "kept {amount} {token} from {source} on {host} at {paid_at}: unpriced: {cause}"
            ),
        }
    }
    Ok(())
}

pub(crate) async fn list(host: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let store = crate::queue::JobStorage::new().await?;
    let paths = store
        .list_paths(&format!("{PREFIX}/"), crate::queue::Listing::Every)
        .await?;
    let mut payouts = Vec::new();
    for path in paths {
        // A record removed between the listing and the read is gone, not a
        // payout of zero.
        let Some(text) = store.download_text(&path).await? else {
            continue;
        };
        let payout: TokenPayout = serde_json::from_str(&text).map_err(|error| {
            CmdError::declaration(format!("{path} is not a payout record: {error}"))
        })?;
        if host.is_none_or(|host| payout.host == host) {
            payouts.push(payout);
        }
    }
    let total: f64 = payouts.iter().filter_map(|payout| payout.usd().ok()).sum();
    let unpriced = payouts
        .iter()
        .filter(|payout| payout.usd().is_err())
        .count();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "stado.market-payouts.v1",
                "payouts": payouts,
                "total_usd": total,
                "unpriced": unpriced,
            }))?
        );
        return Ok(());
    }
    for payout in &payouts {
        let value = match payout.usd() {
            Ok(usd) => format!("{usd:.2} USD"),
            Err(cause) => format!("unpriced: {cause}"),
        };
        println!(
            "{}\t{}\t{}\t{} {}\t{value}",
            payout.paid_at, payout.source, payout.host, payout.amount, payout.token
        );
    }
    println!(
        "total {total:.2} USD over {} payout(s){}",
        payouts.len(),
        match unpriced {
            0 => String::new(),
            n => format!(", {n} unpriced"),
        }
    );
    Ok(())
}
