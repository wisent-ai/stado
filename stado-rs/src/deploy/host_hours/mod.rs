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
}

/// One hour of this host.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Hour {
    /// Per board UUID.
    pub boards: BTreeMap<String, BoardHour>,
    pub cpu_samples: u64,
    /// The sum of the processor busy fractions the samples read.
    pub cpu_busy_sum: f64,
    /// Each reading this hour could not take, with its cause, once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unread: Vec<String>,
}

#[derive(Default)]
struct Register {
    /// Hour start (RFC 3339, UTC) to its buckets.
    hours: BTreeMap<String, Hour>,
    /// Per board, when its last power sample was taken.
    last_sample: BTreeMap<String, DateTime<Utc>>,
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

/// The boards' readings now: none on a host without `nvidia-smi`, the
/// driver's refusal when it is installed and fails.
async fn board_samples() -> Result<Vec<BoardSample>, String> {
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
    let mut register = crate::primitives::lock::held(&REGISTER);
    let hour = match hour_of(at) {
        Ok(hour) => hour,
        Err(cause) => {
            tracing::warn!("host hours: this sample is not kept: {cause}");
            return;
        }
    };
    let Register { hours, last_sample } = &mut *register;
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
