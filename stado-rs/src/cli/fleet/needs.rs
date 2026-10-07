//! `stado fleet needs`: what the fleet lacks, from what it publishes.

use chrono::Utc;

use crate::cli::registry::read_registry;
use crate::cli::CmdError;
use crate::fleet_needs::{advise, render};

pub async fn run(json_output: bool, window_days: Option<i64>) -> Result<bool, CmdError> {
    if window_days.is_some_and(|days| days < 1) {
        return Err(CmdError::usage("--days must be at least 1"));
    }
    let registry = read_registry().await?;
    let store = crate::queue::submit::default_store("").await?;
    let report = advise(&store, &registry, window_days, Utc::now())
        .await
        .map_err(CmdError::from)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render::render(&report));
    }
    Ok(true)
}
