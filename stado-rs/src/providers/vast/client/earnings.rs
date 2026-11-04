//! What Vast says a hosted machine earned: `GET
//! /users/me/machine-earnings`, the record its own `vastai show earnings`
//! reads, per machine and per day, gross by kind. Nothing here estimates an
//! amount from a price and a duration: the provider's stated figure is the
//! income, and a day it did not state is absent, never zero.

use serde::Serialize;
use serde_json::Value;

use crate::providers::vast::VastError;

use super::json::json_int;
use super::VastClient;

/// Seconds in one day: Vast's `sday`, `eday` and `day` are whole days since
/// the Unix epoch, as its CLI computes them (`time.time() / Days`).
pub const SECONDS_PER_DAY: i64 = 86_400;

/// One day of one machine as Vast states it, in USD.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EarnedDay {
    /// Days since the Unix epoch.
    pub day: i64,
    pub gpu: f64,
    pub storage: f64,
    pub upload: f64,
    pub download: f64,
}

impl EarnedDay {
    pub fn gross(&self) -> f64 {
        self.gpu + self.storage + self.upload + self.download
    }
}

/// A number Vast states, or why it is not one.
fn amount(row: &Value, key: &str) -> Result<f64, VastError> {
    match row.get(key) {
        Some(Value::Number(number)) => number.as_f64().ok_or_else(|| {
            VastError::config(format!(
                "Vast.ai machine-earnings {key} {number} is not finite"
            ))
        }),
        // A stream a machine did not earn on is absent or null in Vast's
        // answer; that is Vast stating nothing was earned on it.
        Some(Value::Null) | None => Ok(0.0),
        Some(other) => Err(VastError::config(format!(
            "Vast.ai machine-earnings {key} is {other}, not a number"
        ))),
    }
}

impl VastClient {
    /// Each day `machine_id` earned from day `since` through day `until`
    /// (days since the epoch, both included), as Vast states them. Vast is
    /// asked for that one machine, so `per_day` is that machine's.
    pub async fn machine_earnings(
        &self,
        machine_id: i64,
        since: i64,
        until: i64,
    ) -> Result<Vec<EarnedDay>, VastError> {
        let path = format!(
            "/users/me/machine-earnings?owner=me&sday={since}&eday={until}&machid={machine_id}"
        );
        let answer = self.request("GET", &path, None).await?;
        let days = answer
            .get("per_day")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                VastError::config(format!(
                    "Vast.ai {path} answered no per_day list: {}",
                    super::json::shown(Some(&answer))
                ))
            })?;
        days.iter()
            .map(|row| {
                let day = row.get("day").and_then(json_int).ok_or_else(|| {
                    VastError::config(format!(
                        "Vast.ai {path} lists a day with no day number: {row}"
                    ))
                })?;
                Ok(EarnedDay {
                    day,
                    gpu: amount(row, "gpu_earn")?,
                    storage: amount(row, "sto_earn")?,
                    upload: amount(row, "bwu_earn")?,
                    download: amount(row, "bwd_earn")?,
                })
            })
            .collect()
    }
}
