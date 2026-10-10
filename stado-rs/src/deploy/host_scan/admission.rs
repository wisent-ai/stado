//! Whether a scan may run now: the one-minute load average against the
//! host's ceiling.

use nix::libc;
use serde::Serialize;

use crate::capabilities::HOST_SCAN_LOAD_CEILING_CONFIG;

/// Where the ceiling came from, published beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CeilingSource {
    /// `host.scan_load_ceiling`, declared by the operator.
    Declared,
    /// The logical core count this process measured, when nothing is declared.
    MeasuredCores,
}

/// The reading one admission was decided on.
#[derive(Clone, Debug, Serialize)]
pub struct Admission {
    /// The host's one-minute load average when the scan asked.
    pub load: f64,
    pub ceiling: f64,
    pub ceiling_source: CeilingSource,
}

/// The verdict: run, or skip with the reading that refused.
#[derive(Clone, Debug)]
pub enum Verdict {
    Admitted(Admission),
    Skipped(Admission),
}

impl Verdict {
    pub fn admission(&self) -> &Admission {
        match self {
            Self::Admitted(admission) | Self::Skipped(admission) => admission,
        }
    }

    pub fn admitted(&self) -> bool {
        matches!(self, Self::Admitted(_))
    }
}

/// The one-minute load average, from the kernel.
fn load_one_minute() -> Result<f64, String> {
    // One slot, unread until the kernel writes it; a reading that stays
    // unwritten is told apart by not being a number.
    let mut loads = [f64::NAN];
    // SAFETY: `loads` outlives the call and `nelem` is its length.
    let written = unsafe { libc::getloadavg(loads.as_mut_ptr(), loads.len() as libc::c_int) };
    let [load] = loads;
    if written != loads.len() as libc::c_int || load.is_nan() {
        return Err("the kernel answered no load average (getloadavg)".to_string());
    }
    Ok(load)
}

/// The ceiling: declared, or the logical core count this process measures.
fn ceiling() -> Result<(f64, CeilingSource), String> {
    let field = &HOST_SCAN_LOAD_CEILING_CONFIG;
    let declared = crate::config_file::resolve(field.env, field.path, "");
    let declared = declared.trim();
    if !declared.is_empty() {
        let value: f64 = declared.parse().map_err(|error| {
            format!(
                "{} = {declared:?} is not a load average: {error}",
                field.path
            )
        })?;
        if !value.is_finite() || value.is_sign_negative() {
            return Err(format!(
                "{} = {declared:?} is not a load average a host can be under",
                field.path
            ));
        }
        return Ok((value, CeilingSource::Declared));
    }
    let cores = std::thread::available_parallelism()
        .map_err(|error| format!("this host's core count could not be read: {error}"))?;
    Ok((cores.get() as f64, CeilingSource::MeasuredCores))
}

/// Decide whether `scan` runs now. A skip is recorded in [`super::record`]
/// under the scan's name, so the beacon and `stado host health` carry it;
/// an admission clears the scan's earlier skip.
pub fn admit(scan: &str) -> Result<Verdict, String> {
    let load = load_one_minute()?;
    let (ceiling, ceiling_source) = ceiling()?;
    let admission = Admission {
        load,
        ceiling,
        ceiling_source,
    };
    if load > ceiling {
        super::record::skipped(scan, &admission)?;
        Ok(Verdict::Skipped(admission))
    } else {
        super::record::admitted(scan, &admission)?;
        Ok(Verdict::Admitted(admission))
    }
}
