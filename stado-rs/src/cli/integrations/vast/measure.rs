//! `stado market measure --provider vast --gpu NAME [--json]`: what the
//! marketplace asks for a board model right now, kept in the fleet store
//! so what to sell is decided against figures with their time and source.
//!
//! Each reading is kept at `market/prices/vast/<gpu>/<read_at>.json`. The
//! public offer search states prices, not how often boards are rented, so
//! occupancy is printed as not read rather than guessed.

use serde_json::json;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

fn key(gpu: &str, read_at: &str) -> String {
    let slug: String = gpu
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("market/prices/vast/{slug}/{read_at}.json")
}

pub(crate) async fn measure(gpu: &str, json_output: bool) -> Result<(), CmdError> {
    let price = crate::providers::vast::market_prices::read(gpu)
        .await
        .map_err(|error| {
            CmdError::click(format!("the Vast offer search could not be read: {error}"))
                .stating(FailureCode::InfraDown)
        })?;
    if price.offers == 0 {
        return Err(CmdError::missing(format!(
            "Vast lists no rentable one-board on-demand offer of {gpu:?}; the name must be \
             Vast's own spelling of the board (as its offers show it, e.g. \"RTX PRO 6000 WS\")"
        )));
    }
    let store = crate::queue::JobStorage::new().await?;
    let record = json!({
        "schema": "stado.market-price.v1",
        "provider": "vast",
        "price": price,
        "occupancy": null,
        "occupancy_unread": "Vast's public offer search states prices, not how often a board is rented",
    });
    store
        .upload_bytes(
            &key(gpu, &price.read_at),
            serde_json::to_vec(&record)?.as_slice(),
        )
        .await
        .map_err(|error| CmdError::from(error).within(format!("keep the {gpu} price reading")))?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&record)?);
        return Ok(());
    }
    let usd = |value: Option<f64>| match value {
        Some(value) => format!("{value:.3}"),
        None => "none".to_string(),
    };
    println!(
        "{gpu} on Vast at {}: {} offers; renter pays USD/hour min {} median {} max {}; occupancy \
         not read (the offer search states none); kept",
        price.read_at,
        price.offers,
        usd(price.min_usd_per_hour),
        usd(price.median_usd_per_hour),
        usd(price.max_usd_per_hour)
    );
    Ok(())
}
