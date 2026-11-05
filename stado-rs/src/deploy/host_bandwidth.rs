//! What this host's connection carries, measured on the host and published
//! in its beacon (`bandwidth`): the latest download and upload rates in
//! megabits per second, when each was taken, and the cause when one failed.
//!
//! A market offer states the bandwidth a renter gets, and a line measured
//! by nobody was offered as if it were fast. The rates are now measured
//! against the endpoints the host declares, `host.bandwidth_probe` (a
//! `download_url` the host fetches and an `upload_url` it posts to), never a
//! service Stado chose, once per `host.bandwidth_interval_seconds`. Each
//! transfer is bounded by the declared `bytes` (the download stops reading
//! there; the upload sends exactly that many). `stado market readiness`
//! reads the block and refuses an offer whose latest measurement is older
//! than the interval, naming the measurement it lacks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The declared endpoints and size of one measurement.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Probe {
    pub download_url: String,
    pub upload_url: String,
    /// Bytes moved each way per measurement.
    pub bytes: u64,
}

/// One direction's latest measurement.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rate {
    /// Megabits per second, when the transfer completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mbps: Option<f64>,
    /// When the transfer finished or failed (RFC 3339, UTC).
    pub at: String,
    /// Why the transfer failed, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The `bandwidth` block of the beacon.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bandwidth {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download: Option<Rate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload: Option<Rate>,
    /// The declared interval a measurement is current for, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_seconds: Option<u64>,
    /// Why nothing is measured: no probe declared, or the declaration is
    /// unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmeasured: Option<String>,
}

static REGISTER: std::sync::LazyLock<std::sync::Mutex<Bandwidth>> =
    std::sync::LazyLock::new(Default::default);

/// The declared probe and interval, or why there is none.
fn declared() -> Result<(Probe, u64), String> {
    let probe_field = &crate::capabilities::HOST_BANDWIDTH_PROBE_CONFIG;
    let interval_field = &crate::capabilities::HOST_BANDWIDTH_INTERVAL_CONFIG;
    let probe = crate::config_file::field_value(probe_field).ok_or_else(|| {
        format!(
            "{} ({}) is not declared, so this host's bandwidth is not measured: declare \
             {{\"download_url\", \"upload_url\", \"bytes\"}}",
            probe_field.path, probe_field.env
        )
    })?;
    let probe: Probe = match probe {
        Value::String(text) => serde_json::from_str(&text),
        other => serde_json::from_value(other),
    }
    .map_err(|error| {
        format!(
            "{} is not {{\"download_url\", \"upload_url\", \"bytes\"}}: {error}",
            probe_field.path
        )
    })?;
    if probe.bytes == 0 {
        return Err(format!("{}.bytes must be above zero", probe_field.path));
    }
    for url in [&probe.download_url, &probe.upload_url] {
        url::Url::parse(url)
            .map_err(|error| format!("{}: {url} is not a URL: {error}", probe_field.path))?;
    }
    let interval = crate::config_file::field_value(interval_field)
        .and_then(|value| match value {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.trim().parse().ok(),
            _ => None,
        })
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            format!(
                "{} ({}) must declare, in seconds above zero, how often the bandwidth is measured",
                interval_field.path, interval_field.env
            )
        })?;
    Ok((probe, interval))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Megabits per second for `bytes` moved in `seconds`.
fn mbps(bytes: u64, seconds: f64) -> Option<f64> {
    (seconds > 0.0).then(|| bytes as f64 * 8.0 / seconds / 1_000_000.0)
}

async fn download(client: &reqwest::Client, probe: &Probe) -> Rate {
    let started = std::time::Instant::now();
    let outcome: Result<u64, String> = async {
        let response = crate::wait::request(client.get(&probe.download_url))
            .await
            .map_err(|error| format!("{} did not answer: {error}", probe.download_url))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("{} answered {status}", probe.download_url));
        }
        let mut response = response;
        let mut read: u64 = 0;
        while read < probe.bytes {
            match response.chunk().await {
                Ok(Some(chunk)) => read += chunk.len() as u64,
                Ok(None) => break,
                Err(error) => {
                    return Err(format!("{} stopped sending: {error}", probe.download_url))
                }
            }
        }
        if read < probe.bytes {
            return Err(format!(
                "{} sent {read} bytes, fewer than the {} declared",
                probe.download_url, probe.bytes
            ));
        }
        Ok(read)
    }
    .await;
    rate(outcome, started.elapsed().as_secs_f64())
}

async fn upload(client: &reqwest::Client, probe: &Probe) -> Rate {
    let body = vec![0u8; usize::try_from(probe.bytes).unwrap_or(usize::MAX)];
    let started = std::time::Instant::now();
    let outcome: Result<u64, String> = async {
        let response = crate::wait::request(client.post(&probe.upload_url).body(body))
            .await
            .map_err(|error| format!("{} did not take the upload: {error}", probe.upload_url))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("{} answered {status}", probe.upload_url));
        }
        Ok(probe.bytes)
    }
    .await;
    rate(outcome, started.elapsed().as_secs_f64())
}

fn rate(outcome: Result<u64, String>, seconds: f64) -> Rate {
    match outcome {
        Ok(bytes) => Rate {
            mbps: mbps(bytes, seconds),
            at: now(),
            error: None,
        },
        Err(error) => Rate {
            mbps: None,
            at: now(),
            error: Some(error),
        },
    }
}

/// Measure both directions when the latest measurement is older than the
/// declared interval; record why nothing is measured when no probe is
/// declared. Called once per beacon period.
pub async fn sample() {
    let (probe, interval) = match declared() {
        Ok(declared) => declared,
        Err(cause) => {
            let mut register = crate::primitives::lock::held(&REGISTER);
            register.unmeasured = Some(cause);
            return;
        }
    };
    let due = {
        let register = crate::primitives::lock::held(&REGISTER);
        register
            .download
            .as_ref()
            .and_then(|rate| chrono::DateTime::parse_from_rfc3339(&rate.at).ok())
            .is_none_or(|at| {
                (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds()
                    >= i64::try_from(interval).unwrap_or(i64::MAX)
            })
    };
    if !due {
        return;
    }
    let client = reqwest::Client::new();
    let down = download(&client, &probe).await;
    let up = upload(&client, &probe).await;
    let mut register = crate::primitives::lock::held(&REGISTER);
    register.download = Some(down);
    register.upload = Some(up);
    register.interval_seconds = Some(interval);
    register.unmeasured = None;
}

/// The `bandwidth` block of this host's beacon.
pub fn published() -> Result<Value, String> {
    let register = crate::primitives::lock::held(&REGISTER);
    serde_json::to_value(&*register)
        .map_err(|error| format!("the bandwidth block cannot be serialized: {error}"))
}

/// Whether a published block carries a current measurement in both
/// directions, and if not, why: what a market offer is refused for.
pub fn current(block: &Value) -> Result<(f64, f64), String> {
    let bandwidth: Bandwidth = serde_json::from_value(block.clone())
        .map_err(|error| format!("the beacon's bandwidth block is unreadable: {error}"))?;
    if let Some(cause) = &bandwidth.unmeasured {
        return Err(cause.clone());
    }
    let interval = bandwidth
        .interval_seconds
        .ok_or("the beacon carries no bandwidth measurement yet")?;
    let reading = |name: &str, rate: &Option<Rate>| -> Result<f64, String> {
        let rate = rate
            .as_ref()
            .ok_or_else(|| format!("no {name} measurement is published"))?;
        if let Some(error) = &rate.error {
            return Err(format!(
                "the latest {name} measurement at {} failed: {error}",
                rate.at
            ));
        }
        let at = chrono::DateTime::parse_from_rfc3339(&rate.at).map_err(|error| {
            format!(
                "the {name} measurement time {} is unreadable: {error}",
                rate.at
            )
        })?;
        let age = (chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds();
        if age > i64::try_from(interval).unwrap_or(i64::MAX) {
            return Err(format!(
                "the latest {name} measurement is {age} s old, past the declared {interval} s"
            ));
        }
        rate.mbps
            .ok_or_else(|| format!("the latest {name} measurement states no rate"))
    };
    Ok((
        reading("download", &bandwidth.download)?,
        reading("upload", &bandwidth.upload)?,
    ))
}
