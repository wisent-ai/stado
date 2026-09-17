//! `stado fleet needs`: what the fleet lacks, from what it publishes.

use chrono::Utc;

use crate::cli::registry::read_registry;
use crate::fleet_needs::{advise, render};

pub async fn run(json_output: bool, window_days: i64) -> Result<bool, String> {
    if window_days < 1 {
        return Err("--days must be at least 1".to_string());
    }
    let registry = read_registry().await.map_err(|error| error.to_string())?;
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| error.to_string())?;
    let report = advise(&store, &registry, window_days, Utc::now())
        .await
        .map_err(|error| error.to_string())?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        print!("{}", render::render(&report));
    }
    Ok(true)
}
