//! What each hour of this host cost in work and power, measured on the host
//! and published in its beacon (`hours`): per board, the share of samples it
//! was busy and the energy it drew; for the processor, its busy share.
//!
//! Owned hardware was priced by one hand-written hourly figure for every host
//! and board, busy or idle, and nothing measured power: the agent read the
//! boards' memory and power cap but never their draw or utilization. An hour
//! is now measured where it happens, every sample kept as a running sum in
//! the hour's bucket, so a reader prices the energy the boards drew at the
//! host's declared tariff (`stado cost host`), and an hour with no sample is
//! said to be unmeasured, never read as idle or free.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, DurationRound, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How many past hours a beacon carries: one day, the window `stado cost
/// host` reads by default, so a host publishing every period keeps a full
/// day readable without a store of its own.
const HOURS_KEPT: i64 = 24;

/// One board's samples within an hour.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BoardHour {
    pub samples: u64,
    /// Samples whose utilization.gpu was above zero.
    pub busy_samples: u64,
    /// Watt-seconds drawn: each sample's power.draw times the seconds since
    /// the previous sample of the same board, so the sum is the energy.
    pub watt_seconds: f64,
    /// The part of `watt_seconds` drawn in samples whose utilization.gpu was
    /// zero, and the seconds those samples cover: the board's idle draw,
    /// which it spends whether or not anything runs, so a sale or job is
    /// charged only what it drew above it.
    #[serde(default)]
    pub idle_watt_seconds: f64,
    #[serde(default)]
    pub idle_seconds: f64,
    /// The seconds `watt_seconds` covers, so a reader charges the idle draw
    /// over exactly the time the energy was measured.
    #[serde(default)]
    pub seconds: f64,
}

/// One hour of this host.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Hour {
    /// Per board UUID.
    pub boards: BTreeMap<String, BoardHour>,
    pub cpu_samples: u64,
    /// The sum of the processor busy fractions the samples read.
    pub cpu_busy_sum: f64,
    /// What this host's inference deployment served in the hour, from its
    /// engine's own counters; absent when no deployment runs here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served: Option<Served>,
    /// Each reading this hour could not take, with its cause, once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<String>,
}

/// Tokens and requests one deployment served within an hour.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Served {
    pub deployment: String,
    pub prompt_tokens: f64,
    pub generation_tokens: f64,
    pub requests: f64,
}

/// The engine's cumulative counters at one reading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Counters {
    prompt_tokens: f64,
    generation_tokens: f64,
    requests: f64,
}

#[derive(Default)]
struct Register {
    /// Hour start (RFC 3339, UTC) to its buckets.
    hours: BTreeMap<String, Hour>,
    /// Per board, when its last power sample was taken.
    last_sample: BTreeMap<String, DateTime<Utc>>,
    /// The engine's counters at the previous reading, per deployment.
    last_served: BTreeMap<String, Counters>,
}

/// The sum of every sample of `name` in a Prometheus text exposition (all
/// label sets), or `None` when the exposition carries no such metric.
fn metric_sum(exposition: &str, name: &str) -> Option<f64> {
    let mut found = None;
    for line in exposition.lines() {
        if line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        // The name ends at a label set or the value; a longer metric that
        // shares the prefix is another metric.
        if !(rest.starts_with('{') || rest.starts_with(' ')) {
            continue;
        }
        if let Some(value) = rest.rsplit(' ').next().and_then(|v| v.parse::<f64>().ok()) {
            *found.get_or_insert(0.0) += value;
        }
    }
    found
}

/// The served counters of this host's inference deployment, read from its
/// engine's `/metrics` (vLLM's `vllm:prompt_tokens_total`,
/// `vllm:generation_tokens_total` and `vllm:request_success_total`); `None`
/// when no deployment is reserved here.
async fn served_counters() -> Result<Option<(String, Counters)>, String> {
    let Some(active) = crate::inference::reservation::active()? else {
        return Ok(None);
    };
    let url = format!("http://{}:{}/metrics", active.endpoint_host, active.port);
    let response = crate::wait::request(reqwest::Client::new().get(&url))
        .await
        .map_err(|error| format!("{url} did not answer: {error}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        format!("{url} answered {status} and its body could not be read: {error}")
    })?;
    if !status.is_success() {
        return Err(format!("{url} answered {status}"));
    }
    let read =
        |name: &str| metric_sum(&body, name).ok_or_else(|| format!("{url} carries no {name}"));
    Ok(Some((
        active.deployment,
        Counters {
            prompt_tokens: read("vllm:prompt_tokens_total")?,
            generation_tokens: read("vllm:generation_tokens_total")?,
            requests: read("vllm:request_success_total")?,
        },
    )))
}

static REGISTER: std::sync::LazyLock<std::sync::Mutex<Register>> =
    std::sync::LazyLock::new(Default::default);

/// One board's reading.
#[derive(Debug, PartialEq)]
pub struct BoardSample {
    pub uuid: String,
    pub utilization_percent: f64,
    pub power_watts: f64,
}

/// Rows of `nvidia-smi --query-gpu=uuid,utilization.gpu,power.draw
/// --format=csv,noheader,nounits`. A row this cannot read is refused naming
/// it: dropped, its board's hour would read as drawing nothing.
pub fn parse_board_samples(stdout: &str) -> Result<Vec<BoardSample>, String> {
    let mut samples = Vec::new();
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let [uuid, utilization, power] = fields.as_slice() else {
            return Err(format!(
                "nvidia-smi printed {line:?}, not uuid, utilization.gpu, power.draw"
            ));
        };
        let number = |name: &str, value: &str| {
            value.parse::<f64>().map_err(|error| {
                format!("nvidia-smi's {name} {value:?} for {uuid} is not a number: {error}")
            })
        };
        samples.push(BoardSample {
            uuid: uuid.to_string(),
            utilization_percent: number("utilization.gpu", utilization)?,
            power_watts: number("power.draw", power)?,
        });
    }
    Ok(samples)
}

/// The boards' readings now: the NVIDIA boards `nvidia-smi` reports, and on
/// macOS the package's draw from the declared power helper
/// ([`package_sample`]); none on another host without `nvidia-smi`, the
/// driver's refusal when it is installed and fails.
async fn board_samples() -> Result<Vec<BoardSample>, String> {
    if cfg!(target_os = "macos") {
        return package_sample().await.map(|sample| vec![sample]);
    }
    let output = match crate::wait::output_async(tokio::process::Command::new("nvidia-smi").args([
        "--query-gpu=uuid,utilization.gpu,power.draw",
        "--format=csv,noheader,nounits",
    ]))
    .await
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("nvidia-smi could not run: {error}")),
    };
    if !output.status.success() {
        return Err(format!(
            "nvidia-smi exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_board_samples(&String::from_utf8_lossy(&output.stdout))
}

/// The board key a Mac's whole package is kept under: CPU, GPU and Neural
/// Engine share one power domain, so the package is the one board an hour
/// prices.
pub const MACOS_PACKAGE: &str = "macos-package";

/// One sample of a Mac's package from the declared power helper
/// (`host.power_helper`): its combined draw (`processor.combined_power`,
/// milliwatts) and, from the graphics sampler, whether the GPU was busy
/// (`gpu.idle_ratio` below one). The helper is declared because
/// `powermetrics` reads power only as root; an undeclared helper, a helper
/// that fails, and a sample without those keys are each refused naming what
/// is missing, so the hour says why this Mac's draw is absent.
async fn package_sample() -> Result<BoardSample, String> {
    let field = &crate::capabilities::HOST_POWER_HELPER_CONFIG;
    let words: Vec<String> = match crate::config_file::field_value(field) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(text)) => text.split_whitespace().map(str::to_string).collect(),
        Some(other) => {
            return Err(format!(
                "{} is {other}, not the helper's command words",
                field.path
            ))
        }
        None => Vec::new(),
    };
    let Some((program, args)) = words.split_first() else {
        return Err(format!(
            "{} ({}) is not declared, so this Mac's power is not read: declare the privileged helper \
             that prints one `powermetrics --samplers cpu_power,gpu_power -n 1 -f plist` sample",
            field.path, field.env
        ));
    };
    let output = crate::wait::output_async(tokio::process::Command::new(program).args(args))
        .await
        .map_err(|error| format!("the power helper {program} could not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "the power helper {program} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_package_sample(&output.stdout)
}

/// One `powermetrics -f plist` sample as [`package_sample`] reads it.
pub fn parse_package_sample(stdout: &[u8]) -> Result<BoardSample, String> {
    // powermetrics ends each plist sample with a NUL byte.
    let end = stdout
        .iter()
        .rposition(|byte| *byte != 0 && !byte.is_ascii_whitespace())
        .map_or(0, |at| at + 1);
    let sample = plist::Value::from_reader(std::io::Cursor::new(&stdout[..end]))
        .map_err(|error| format!("the power helper printed no plist sample: {error}"))?;
    let dict = sample
        .as_dictionary()
        .ok_or("the power helper's sample is not a dictionary")?;
    let number = |section: &str, key: &str| -> Result<f64, String> {
        let value = dict
            .get(section)
            .and_then(plist::Value::as_dictionary)
            .and_then(|inner| inner.get(key))
            .ok_or_else(|| format!("the power helper's sample has no {section}.{key}"))?;
        value
            .as_real()
            .or_else(|| value.as_signed_integer().map(|integer| integer as f64))
            .or_else(|| value.as_unsigned_integer().map(|integer| integer as f64))
            .ok_or_else(|| format!("the power helper's {section}.{key} is not a number"))
    };
    let milliwatts = number("processor", "combined_power")?;
    let idle_ratio = number("gpu", "idle_ratio")?;
    Ok(BoardSample {
        uuid: MACOS_PACKAGE.to_string(),
        utilization_percent: ((1.0 - idle_ratio) * 100.0).max(0.0),
        power_watts: milliwatts / 1000.0,
    })
}

fn hour_of(at: DateTime<Utc>) -> Result<String, String> {
    at.duration_trunc(Duration::hours(1))
        .map(|start| start.to_rfc3339_opts(SecondsFormat::Secs, true))
        .map_err(|error| format!("{at} cannot be cut to its hour: {error}"))
}

/// Take one sample of every board and the processor into the current hour,
/// and drop hours older than the window a beacon carries. Each reading that
/// fails is kept once in the hour's `unread`, so the hour says why it is
/// short instead of reading as idle.
pub async fn sample() {
    let at = Utc::now();
    let boards = board_samples().await;
    let cpu = crate::providers::local::helpers::host::cpu::busy_fraction();
    let served = served_counters().await;
    let mut register = crate::primitives::lock::held(&REGISTER);
    let hour = match hour_of(at) {
        Ok(hour) => hour,
        Err(cause) => {
            tracing::warn!("host hours: this sample is not kept: {cause}");
            return;
        }
    };
    let Register {
        hours,
        last_sample,
        last_served,
    } = &mut *register;
    let bucket = hours.entry(hour).or_default();
    let mut note = |bucket: &mut Hour, cause: String| {
        if !bucket.unread.contains(&cause) {
            bucket.unread.push(cause);
        }
    };
    match boards {
        Ok(boards) => {
            for board in boards {
                let seconds = last_sample
                    .insert(board.uuid.clone(), at)
                    .map(|previous| (at - previous).num_milliseconds() as f64 / 1000.0);
                let entry = bucket.boards.entry(board.uuid).or_default();
                entry.samples += 1;
                if board.utilization_percent > 0.0 {
                    entry.busy_samples += 1;
                }
                // The first sample of a board has no interval behind it, so
                // it adds no energy; every later one adds its draw over the
                // seconds since the one before.
                if let Some(seconds) = seconds.filter(|seconds| *seconds > 0.0) {
                    entry.watt_seconds += board.power_watts * seconds;
                    entry.seconds += seconds;
                    if board.utilization_percent <= 0.0 {
                        entry.idle_watt_seconds += board.power_watts * seconds;
                        entry.idle_seconds += seconds;
                    }
                }
            }
        }
        Err(cause) => note(bucket, format!("boards: {cause}")),
    }
    match cpu {
        Ok(Some(fraction)) => {
            bucket.cpu_samples += 1;
            bucket.cpu_busy_sum += fraction;
        }
        Ok(None) => {}
        Err(cause) => note(bucket, format!("processor: {cause}")),
    }
    match served {
        Ok(Some((deployment, now))) => {
            // The engine's counters only grow; one that fell was restarted,
            // and what it served since is its new total.
            if let Some(before) = last_served.insert(deployment.clone(), now) {
                let delta = |now: f64, before: f64| if now >= before { now - before } else { now };
                let entry = bucket.served.get_or_insert_with(|| Served {
                    deployment: deployment.clone(),
                    ..Served::default()
                });
                entry.prompt_tokens += delta(now.prompt_tokens, before.prompt_tokens);
                entry.generation_tokens += delta(now.generation_tokens, before.generation_tokens);
                entry.requests += delta(now.requests, before.requests);
            }
        }
        Ok(None) => {}
        Err(cause) => note(bucket, format!("served: {cause}")),
    }
    let oldest = at - Duration::hours(HOURS_KEPT);
    if let Ok(oldest) = hour_of(oldest) {
        hours.retain(|hour, _| *hour >= oldest);
    }
}

/// The `hours` block of this host's beacon: each kept hour by its start.
pub fn published() -> Result<Value, String> {
    let register = crate::primitives::lock::held(&REGISTER);
    serde_json::to_value(&register.hours)
        .map_err(|error| format!("the host hours cannot be serialized: {error}"))
}

/// Joules in one kilowatt-hour: 1000 W for 3600 s (SI).
pub const JOULES_PER_KWH: f64 = 3_600_000.0;

/// Each board's idle draw in watts over every hour of a beacon's `hours`
/// block: the energy of its zero-utilization samples over the seconds they
/// cover. A board with no idle sample in the window has no measured idle
/// draw and is left out. Every reader of owned cost (`stado cost host`, the
/// scheduler's job pricing, `stado market earnings`) charges work only what
/// a board drew above this, because a board draws it with nothing running.
pub fn idle_watts(hours: &serde_json::Map<String, Value>) -> BTreeMap<String, f64> {
    let mut sums: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for hour in hours.values() {
        for (uuid, board) in hour["boards"].as_object().into_iter().flatten() {
            let sum = sums.entry(uuid.clone()).or_default();
            sum.0 += board["idle_watt_seconds"].as_f64().unwrap_or(0.0);
            sum.1 += board["idle_seconds"].as_f64().unwrap_or(0.0);
        }
    }
    sums.into_iter()
        .filter(|(_, (_, seconds))| *seconds > 0.0)
        .map(|(uuid, (energy, seconds))| (uuid, energy / seconds))
        .collect()
}

/// The kWh board `uuid` drew above its idle draw in one hour's `board`
/// entry, or `None` when its idle draw is unmeasured or the entry predates
/// covered seconds: the whole draw is never charged in its place.
pub fn kwh_above_idle(uuid: &str, board: &Value, idle: &BTreeMap<String, f64>) -> Option<f64> {
    let watts = idle.get(uuid)?;
    let seconds = board["seconds"].as_f64()?;
    let energy = board["watt_seconds"].as_f64().unwrap_or(0.0);
    Some((energy - watts * seconds).max(0.0) / JOULES_PER_KWH)
}
