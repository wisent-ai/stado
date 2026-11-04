//! `stado market serve-or-rent --target HOST --outside-model ID --gpu BOARD
//! [--since-hours N] [--json]`: whether the inference deployment on a host
//! is worth more than renting the host's board out.
//!
//! What a deployment is worth is what its traffic would have cost bought
//! from outside: the tokens the engine served, measured per hour in the
//! host's beacon (`hours[].served`), priced at the per-token prices of the
//! outside model the operator names exactly as OpenRouter lists it (an
//! aggregator's id differs from the repository, and a version guessed from
//! one would price another model). What renting is worth is the median
//! hourly price renters pay for the board model, from the newest `stado
//! market measure` reading, before Vast's share (not published per offer,
//! so the figure says so). Both are per hour; the larger is named. Every
//! figure that cannot be read is said, and no comparison is made on a guess.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// The outside model's per-token prices, from the provider's own model list.
struct OutsidePrice {
    model: String,
    usd_per_prompt_token: f64,
    usd_per_completion_token: f64,
    source: String,
}

/// The per-token prices of OpenRouter model `model` (its exact id, as its
/// public model list names it).
async fn outside_price(model: &str) -> Result<OutsidePrice, String> {
    let url = "https://openrouter.ai/api/v1/models";
    let response = crate::wait::request(reqwest::Client::new().get(url))
        .await
        .map_err(|error| format!("{url} did not answer: {error}"))?;
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("{url} answered no JSON: {error}"))?;
    let wanted = model.to_string();
    let entry = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| {
            entry["id"]
                .as_str()
                .is_some_and(|id| id.eq_ignore_ascii_case(&wanted))
        })
        .ok_or_else(|| format!("{url} lists no model {wanted}"))?;
    let price = |field: &str| {
        entry["pricing"][field]
            .as_str()
            .and_then(|text| text.parse::<f64>().ok())
            .ok_or_else(|| format!("{url} states no {field} price for {wanted}"))
    };
    Ok(OutsidePrice {
        model: wanted.clone(),
        usd_per_prompt_token: price("prompt")?,
        usd_per_completion_token: price("completion")?,
        source: url.to_string(),
    })
}

/// The newest kept `stado market measure` reading of `gpu`.
async fn rental_price(
    store: &crate::queue::JobStorage,
    gpu: &str,
) -> Result<(f64, String), String> {
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
    let prefix = format!("market/prices/vast/{slug}/");
    let paths = store
        .list_paths(&prefix, crate::queue::Listing::Every)
        .await
        .map_err(|error| format!("the kept market readings cannot be listed: {error}"))?;
    let newest = paths
        .iter()
        .max()
        .ok_or_else(|| format!("no market reading of {gpu} is kept; run `stado market measure --provider vast --gpu \"{gpu}\"`"))?;
    let text = store
        .download_text(newest)
        .await
        .map_err(|error| format!("{newest} cannot be read: {error}"))?
        .ok_or_else(|| format!("{newest} vanished while it was read"))?;
    let record: Value =
        serde_json::from_str(&text).map_err(|error| format!("{newest} is not JSON: {error}"))?;
    let median = record["price"]["median_usd_per_hour"]
        .as_f64()
        .ok_or_else(|| format!("{newest} carries no median price"))?;
    let read_at = record["price"]["read_at"]
        .as_str()
        .unwrap_or("?")
        .to_string();
    Ok((median, read_at))
}

pub(crate) async fn serve_or_rent(
    target: &str,
    outside_model: &str,
    gpu: &str,
    since_hours: u32,
    json_output: bool,
) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = crate::targets::load_registry_from_value(&document)
        .map_err(|error| CmdError::declaration(format!("the registry does not load: {error}")))?;
    let host = crate::cli::resolved_host(&registry, target)?;
    let store = crate::queue::JobStorage::new().await?;
    let beacons = crate::cli::registry::beacon_bodies(&store).await?;
    let slugs = crate::monitor::host_health::beacon_slugs(host, &host.name);
    let hours = slugs
        .iter()
        .find_map(|slug| beacons.get(slug))
        .and_then(|beacon| beacon.get("hours"))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::missing(format!("{}'s beacon carries no measured hours", host.name))
        })?;
    let since = (chrono::Utc::now() - chrono::Duration::hours(i64::from(since_hours)))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let window: Vec<&Value> = hours
        .iter()
        .filter(|(start, _)| start.as_str() >= since.as_str())
        .map(|(_, hour)| hour)
        .collect();
    let measured: Vec<&Value> = window
        .iter()
        .copied()
        .filter(|hour| hour.get("served").is_some_and(|served| !served.is_null()))
        .collect();
    if measured.is_empty() {
        return Err(CmdError::missing(format!(
            "{} measured no served tokens in the last {since_hours} hour(s): its beacon predates \
             served-token measurement, or no deployment answered its engine's /metrics",
            host.name
        )));
    }
    let sum = |field: &str| -> f64 {
        measured
            .iter()
            .filter_map(|hour| hour["served"][field].as_f64())
            .sum()
    };
    let (prompt, generation, requests) = (
        sum("prompt_tokens"),
        sum("generation_tokens"),
        sum("requests"),
    );
    let hours_measured = measured.len() as f64;
    let outside = outside_price(outside_model).await.map_err(|cause| {
        CmdError::click(format!("the outside price cannot be read: {cause}"))
            .stating(FailureCode::InfraDown)
    })?;
    let serving_per_hour = (prompt * outside.usd_per_prompt_token
        + generation * outside.usd_per_completion_token)
        / hours_measured;
    let (rental_per_hour, rental_read_at) =
        rental_price(&store, gpu).await.map_err(CmdError::missing)?;
    let larger = if serving_per_hour >= rental_per_hour {
        "serving"
    } else {
        "renting"
    };
    let report = json!({
        "schema": "stado.market-serve-or-rent.v1",
        "host": host.name,
        "hours_measured": hours_measured,
        "served": {"prompt_tokens": prompt, "generation_tokens": generation, "requests": requests},
        "outside": {"model": outside.model, "usd_per_prompt_token": outside.usd_per_prompt_token,
                    "usd_per_completion_token": outside.usd_per_completion_token, "source": outside.source},
        "serving_usd_per_hour": serving_per_hour,
        "renting_usd_per_hour": rental_per_hour,
        "renting_read_at": rental_read_at,
        "renting_basis": "the median a renter pays per hour for this board on Vast, before Vast's share",
        "larger": larger,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "{} over {hours_measured} measured hour(s): served {prompt} prompt and {generation} \
         generated tokens in {requests} requests",
        host.name
    );
    println!(
        "serving is worth {serving_per_hour:.3} USD/hour at {}'s price on {}",
        outside.model, outside.source
    );
    println!(
        "renting {gpu} is worth {rental_per_hour:.3} USD/hour (median renter price read at \
         {rental_read_at}, before Vast's share)"
    );
    println!("{larger} is worth more");
    Ok(())
}
