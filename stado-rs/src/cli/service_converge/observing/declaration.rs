//! What is declared: the binaries TARGET names a managed version for.

use crate::deploy::host_release;
use crate::targets::ComputeTarget;

use crate::cli::CmdError;

// ---------------------------------------------------------------------------
// What is declared
// ---------------------------------------------------------------------------

/// The binaries TARGET declares a version for, narrowed by BINARY.
///
/// Read straight off `targets[].managed_versions` through
/// [`ComputeTarget::declared_version`], the same accessor the release delivery
/// judges against: two readings of the declaration that can disagree turn
/// "the host is behind" and "the delivery is refused" into independent
/// answers to one question.
///
/// A declared version that is not an exact semantic version is refused here,
/// before the host is contacted at all, and so is a key someone emptied instead
/// of removing. Delivery refuses either one, so a comparison against them could
/// only ever produce drift no command in this pack can close.
pub(in crate::cli::service_converge) fn declaring(
    target: &ComputeTarget,
    binary: Option<&str>,
) -> Result<Vec<(String, String)>, CmdError> {
    let declared: Vec<(String, String)> = target
        .managed_versions
        .iter()
        .filter(|(name, _)| binary.is_none_or(|query| *name == query))
        .map(|(name, version)| (name.clone(), version.clone()))
        .collect();
    if declared.is_empty() {
        if let Some(query) = binary {
            return Err(CmdError::click(format!(
                "{} declares no {query} version; add it to targets[].managed_versions with \
                 `stado release declare-version --host {} --binary {query} --version X.Y.Z`",
                target.name, target.name
            )));
        }
        return Ok(declared);
    }
    for (name, version) in &declared {
        if !host_release::is_exact_semver(version) {
            return Err(CmdError::click(format!(
                "{} declares {name} version {version:?}, which is not an exact semantic version; \
                 set targets[].managed_versions.{name} to a version such as 0.5.1",
                target.name
            )));
        }
    }
    Ok(declared)
}
