//! What the answer means: the exit status, and the two clauses that need a
//! sentence rather than a number.

use crate::cli::CmdError;

/// GiB with one decimal, or a dash for a number this host did not answer with.
pub(super) fn gigabytes(value: Option<f64>) -> String {
    value.map_or_else(|| "not observed".to_string(), |gb| format!("{gb} GiB"))
}

/// What the two backend names mean when they do not agree, read off the
/// blocker [`crate::deploy::host_gates`] already decided.
///
/// Keyed off the blocker and never re-classified here: a second classifier of
/// storage backends in the CLI would eventually disagree with the one in the
/// reader about one host, and the operator would believe whichever line they
/// read first.
pub(super) fn store_clause(blockers: &[String]) -> &'static str {
    if blockers
        .iter()
        .any(|blocker| blocker == crate::deploy::host_gates::AGENT_STORE_DEVICE_ONLY)
    {
        return " — a store only that host can address, so nothing its agent publishes ever \
                reaches this fleet";
    }
    if blockers
        .iter()
        .any(|blocker| blocker == crate::deploy::host_gates::AGENT_STORE_UNKNOWN)
    {
        return " — a backend this build has no adapter for, so how far that agent's writes \
                carry cannot be decided here";
    }
    ""
}

/// A host that is not claiming is a failed verdict, not a failed command: the
/// read succeeded either way, and the message names the blockers rather than
/// repeating that something is wrong.
pub(super) fn claiming_outcome(
    gates: &crate::deploy::host_gates::HostGates,
) -> Result<(), CmdError> {
    if !gates.complete {
        let details = gates
            .observations
            .iter()
            .filter(|read| !read.complete())
            .map(|read| {
                format!(
                    "{}: {}",
                    read.operation,
                    read.detail.as_deref().unwrap_or("no result")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let failure = CmdError::click(format!(
            "{} diagnostic is incomplete: {details}",
            gates.host
        ));
        if gates
            .observations
            .iter()
            .any(|read| read.state == crate::deploy::host_gates::ReadState::TimedOut)
        {
            return Err(failure.stating(crate::primitives::failure::FailureCode::Timeout));
        }
        return Err(failure);
    }
    if gates.claiming {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{} is claiming nothing: {}",
        gates.host,
        gates.blockers.join(", ")
    )))
}

/// Which declared disk threshold `--require-disk` holds the host to.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum DiskRequirement {
    /// The watermark admission is gated on.
    LowWatermark,
    /// The free space reclamation aims for.
    Target,
}

/// The disk verdict alone: free space against the chosen declared threshold,
/// whatever else the host is blocked on. A threshold or reading the host did
/// not answer with is a failure, never a pass.
pub(super) fn disk_outcome(
    gates: &crate::deploy::host_gates::HostGates,
    requirement: DiskRequirement,
) -> Result<(), CmdError> {
    let (name, threshold) = match requirement {
        DiskRequirement::LowWatermark => ("low_watermark_gb", gates.low_watermark_gb),
        DiskRequirement::Target => ("target_free_gb", gates.target_free_gb),
    };
    let (Some(free), Some(threshold)) = (gates.free_gb, threshold) else {
        return Err(CmdError::click(format!(
            "{} did not report free_gb and {name}",
            gates.host
        )));
    };
    if free < threshold as f64 {
        return Err(CmdError::click(format!(
            "{} disk is below {name}: {free:.2} < {threshold} GiB",
            gates.host
        )));
    }
    println!("{} disk meets {name}: {free:.2} >= {threshold} GiB", gates.host);
    Ok(())
}
