//! What the health-beacon scripts need to know before they publish or relay,
//! answered by the product instead of by a parser in the script.
//!
//! - `stado host beacon-coordinates --host SLUG` prints the health API this
//!   host addresses (its configured Stado object store) and the Skarbiec
//!   endpoint the registry's service directory declares for SLUG, tab
//!   separated; either field is empty when nothing declares it.
//! - `stado host beacon-stale --api-url URL --token-file FILE
//!   --fresh-seconds N` prints the registry targets nobody is reporting for,
//!   space separated: no beacon under the target's name or any hostname it
//!   declares is younger than N seconds. It asks the store the beacon
//!   publishes to, never a same-disk mirror, so a hiccup in the fleet
//!   endpoint cannot make a healthy host look stale and get spoken over.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::cli::CmdError;

pub async fn beacon_coordinates(host: &str) -> Result<(), CmdError> {
    let api = crate::config::wc_stado_storage_url();
    let document = crate::cli::registry::fetch_document().await.ok();
    let skarbiec = document
        .as_ref()
        .and_then(|document| {
            document
                .pointer("/service_directory/services/skarbiec/endpoints")?
                .get(host)?
                .get("url")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default();
    println!("{api}\t{skarbiec}");
    Ok(())
}

/// Seconds since the beacon stored under `slug` says it was reported, or
/// `None` when there is no such beacon or it cannot be read.
async fn beacon_age(client: &reqwest::Client, base: &str, token: &str, slug: &str) -> Option<f64> {
    let uri = format!("stado://probierz/host_health/{slug}.json");
    let mut url = url::Url::parse(&format!("{base}/api/object")).ok()?;
    url.query_pairs_mut().append_pair("uri", &uri);
    let response = client.get(url).bearer_auth(token).send().await.ok()?;
    let body: Value = response.error_for_status().ok()?.json().await.ok()?;
    let stamp = body.get("reported_at")?.as_str()?;
    let reported = DateTime::parse_from_rfc3339(stamp).ok()?;
    Some((Utc::now() - reported.with_timezone(&Utc)).num_milliseconds() as f64 / 1000.0)
}

pub async fn beacon_stale(api_url: &str, token_file: &str, fresh_seconds: f64) -> Result<(), CmdError> {
    let base = api_url.trim_end_matches('/');
    let token = std::fs::read_to_string(token_file)
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    let document = crate::cli::registry::fetch_document().await?;
    let client = crate::cli::storage::fleet_https_client()?;
    let mut stale = Vec::new();
    for target in document.get("targets").and_then(Value::as_array).into_iter().flatten() {
        let name = target.get("name").and_then(Value::as_str).unwrap_or("");
        let mut spellings = vec![name.to_string()];
        for hostname in target.get("hostnames").and_then(Value::as_array).into_iter().flatten() {
            let hostname = hostname.as_str().unwrap_or("").to_lowercase();
            let slug = hostname.strip_suffix(".local").unwrap_or(&hostname).to_string();
            if !spellings.contains(&slug) {
                spellings.push(slug);
            }
        }
        let mut youngest: Option<f64> = None;
        for slug in &spellings {
            if let Some(age) = beacon_age(&client, base, &token, slug).await {
                youngest = Some(youngest.map_or(age, |seen: f64| seen.min(age)));
            }
        }
        if youngest.is_none_or(|age| age >= fresh_seconds) {
            stale.push(name.to_string());
        }
    }
    println!("{}", stale.join(" "));
    Ok(())
}
