//! Whether this host is failing in a way a renter would feel, read before
//! each listing and each poll while listed.
//!
//! A contract cannot be ended early, and a machine that crashes, drops off
//! the network or performs badly under one loses reliability score. The
//! signals are the ones this host already measures or declares: each board
//! the driver reports and its uncorrected memory errors, the disk-full rule
//! on the volume it judges, and the measured link against the floor the host
//! declares. A host failing any of them withdraws its offer at once, so no
//! new contract starts on it, and announces maintenance for the contract
//! already running; it is offered again once every reading recovers.

use serde::Deserialize;
use serde_json::Value;

/// Which part of the host is failing: the category a maintenance notice
/// carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Gpu,
    Disk,
    Network,
}

impl Part {
    pub fn category(self) -> &'static str {
        match self {
            Part::Gpu => "gpu",
            Part::Disk => "disk",
            Part::Network => "network",
        }
    }
}

/// What is failing on this host, each with the part it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub causes: Vec<(Part, String)>,
}

impl Failure {
    /// Every cause, in one line.
    pub fn sentence(&self) -> String {
        self.causes
            .iter()
            .map(|(_, cause)| cause.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The part a maintenance notice names: the boards first, since a
    /// renter's work runs there.
    pub fn category(&self) -> &'static str {
        [Part::Gpu, Part::Disk, Part::Network]
            .into_iter()
            .find(|part| self.causes.iter().any(|(each, _)| each == part))
            .unwrap_or(Part::Gpu)
            .category()
    }
}

/// The boards: the driver must answer, report at least one board, and no
/// board may carry an uncorrected volatile ECC error.
async fn boards(causes: &mut Vec<(Part, String)>) {
    match crate::wait::output_async(tokio::process::Command::new("nvidia-smi").args([
        "--query-gpu=uuid,ecc.errors.uncorrected.volatile.total",
        "--format=csv,noheader,nounits",
    ]))
    .await
    {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let rows: Vec<&str> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            if rows.is_empty() {
                causes.push((Part::Gpu, "the NVIDIA driver reports no board".to_string()));
            }
            for row in rows {
                let (uuid, errors) = row.split_once(',').unwrap_or((row, ""));
                // A board without ECC answers "[N/A]": it reports no count,
                // which is not an error count above zero.
                if let Ok(count) = errors.trim().parse::<u64>() {
                    if count > 0 {
                        causes.push((
                            Part::Gpu,
                            format!(
                                "board {} reports {count} uncorrected memory errors since the \
                                 driver loaded",
                                uuid.trim()
                            ),
                        ));
                    }
                }
            }
        }
        Ok(output) => causes.push((
            Part::Gpu,
            format!(
                "the NVIDIA driver failed (nvidia-smi exited {}: {})",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        )),
        Err(error) => causes.push((Part::Gpu, format!("nvidia-smi could not run: {error}"))),
    }
}

/// The disk: the volume the disk-full rule judges must be below its
/// threshold, since a renter's container writes there and the rule would
/// start deleting under it.
fn disk(causes: &mut Vec<(Part, String)>) {
    use crate::providers::local::space_reclaim::rule;
    let home = crate::config_file::expand_tilde("~");
    match rule::read_fleet_volume(&home) {
        Ok(reading) if reading.full() => causes.push((
            Part::Disk,
            format!(
                "the disk {} judges is {:.1}% used, at the disk-full rule's {}% threshold",
                home.display(),
                reading.used_percent(),
                rule::DISK_FULL_PERCENT
            ),
        )),
        Ok(_) => {}
        Err(error) => causes.push((
            Part::Disk,
            format!(
                "the disk the rule judges for {} cannot be read: {error}",
                home.display()
            ),
        )),
    }
}

/// The declared floor, Mbit/s each way.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Floor {
    download: f64,
    upload: f64,
}

/// The link: when the host declares a floor, the current measurement in this
/// host's published beacon must be at or above it each way. A floor that does
/// not parse, or a measurement that is not current, is a failure named as
/// such, since the renter's link could not be vouched for.
async fn link(store: &crate::queue::JobStorage, causes: &mut Vec<(Part, String)>) {
    let field = &crate::capabilities::HOST_BANDWIDTH_FLOOR_CONFIG;
    let Some(declared) = crate::config_file::field_value(field) else {
        return;
    };
    let floor: Result<Floor, String> = match declared {
        Value::String(text) => serde_json::from_str(&text).map_err(|error| error.to_string()),
        other => serde_json::from_value(other).map_err(|error| error.to_string()),
    };
    let floor = match floor {
        Ok(floor) => floor,
        Err(error) => {
            causes.push((
                Part::Network,
                format!(
                    "{} ({}) is not {{\"download\", \"upload\"}} in Mbit/s: {error}",
                    field.path, field.env
                ),
            ));
            return;
        }
    };
    let measured = crate::deploy::host_bandwidth::of_this_host(store)
        .await
        .and_then(|block| crate::deploy::host_bandwidth::current(&block));
    match measured {
        Ok((download, upload)) => {
            for (direction, rate, minimum) in [
                ("download", download, floor.download),
                ("upload", upload, floor.upload),
            ] {
                if rate < minimum {
                    causes.push((
                        Part::Network,
                        format!(
                            "the measured {direction} is {rate:.1} Mbit/s, below the declared \
                             {minimum} Mbit/s floor"
                        ),
                    ));
                }
            }
        }
        Err(cause) => causes.push((
            Part::Network,
            format!("the link has no current measurement against its declared floor: {cause}"),
        )),
    }
}

/// What is failing on this host now, or `None` when every reading holds.
/// `store` is the fleet store this host's beacon is published to.
pub async fn failing(store: &crate::queue::JobStorage) -> Option<Failure> {
    let mut causes = Vec::new();
    boards(&mut causes).await;
    disk(&mut causes);
    link(store, &mut causes).await;
    (!causes.is_empty()).then_some(Failure { causes })
}
