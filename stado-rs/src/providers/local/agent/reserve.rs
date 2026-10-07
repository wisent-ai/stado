//! The admission reserve: how much VRAM and RAM an agent keeps free of the
//! work it admits.
//!
//! The agent refuses to claim a job when admitting it would leave less than
//! this margin between what the device holds and what is in use. It is the
//! last line of defence against a neighbour whose real peak exceeds what it
//! declared — the per-job estimates are under-called by several GiB on
//! activation-extraction workloads — before the next job runs the whole
//! device out of memory.
//!
//! Each margin is the larger of a fraction of the device's total and a floor
//! in GiB, and all four values are the deployment's to state: the fraction
//! and floor once written here (5 % and 4 GiB) were never anyone's decision.
//! An agent whose deployment declares none of them admits no work and says so
//! in its broadcast (`admission_reason: admission_reserve_undeclared`, with
//! `admission_reserve_error` naming the key).

use crate::config_file::resolve as cfg;

/// The four declared values: for VRAM and for RAM, a fraction of the device's
/// total and a floor in GiB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdmissionReserve {
    vram_fraction: f64,
    vram_min_gb: f64,
    ram_fraction: f64,
    ram_min_gb: f64,
}

impl AdmissionReserve {
    /// The reserve this deployment declares, or the sentence naming the first
    /// key it is missing or holds in an unusable form.
    pub fn declared() -> Result<Self, String> {
        Ok(Self {
            vram_fraction: declared(
                "STADO_ADMISSION_VRAM_RESERVE_FRACTION",
                "admission.vram_reserve_fraction",
            )?,
            vram_min_gb: declared(
                "STADO_ADMISSION_VRAM_RESERVE_MIN_GB",
                "admission.vram_reserve_min_gb",
            )?,
            ram_fraction: declared(
                "STADO_ADMISSION_RAM_RESERVE_FRACTION",
                "admission.ram_reserve_fraction",
            )?,
            ram_min_gb: declared(
                "STADO_ADMISSION_RAM_RESERVE_MIN_GB",
                "admission.ram_reserve_min_gb",
            )?,
        })
    }

    /// VRAM kept free on a device of `total_vram_gb`, in whole GiB.
    pub fn vram_gb(&self, total_vram_gb: i64) -> i64 {
        self.vram_min_gb
            .max(total_vram_gb as f64 * self.vram_fraction)
            .ceil() as i64
    }

    /// RAM kept free on a host of `total_ram_gb`.
    pub fn ram_gb(&self, total_ram_gb: f64) -> f64 {
        self.ram_min_gb.max(total_ram_gb * self.ram_fraction)
    }
}

fn declared(env: &str, key: &str) -> Result<f64, String> {
    let raw = cfg(env, key, "");
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(format!(
            "the admission reserve is not declared: {key} (env {env}) is unset, so this agent \
             admits no work; declare it with `stado config set {key} <value>`"
        ));
    }
    let value: f64 = raw
        .parse()
        .map_err(|error| format!("{key} (env {env}) = {raw:?} is not a number: {error}"))?;
    if !value.is_finite() || value.is_sign_negative() {
        return Err(format!(
            "{key} (env {env}) = {raw:?} must be a finite number that is not negative"
        ));
    }
    Ok(value)
}
