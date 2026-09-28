//! What the rows add up to: what is kept, and what makes the command fail.

pub(in crate::cli::service_verify) mod ownership;
pub(in crate::cli::service_verify) mod record;

use crate::cli::CmdError;
use crate::observations::{MISOWNED, STANDBY_SERVING, UNREACHABLE};

use crate::cli::service_verify::Finding;

/// A false declaration is a failure, an unchecked one is not. Exiting non-zero
/// on `unreachable` and `misowned` is what makes this usable as a gate without
/// making an uninstalled probe look like an outage.
///
/// The two failures are counted apart because they send an operator to
/// different places. `unreachable` says nothing answered, and the service is
/// usually what needs attention. `misowned` says the wrong program answered,
/// and the DECLARATION is what needs attention -- restarting the service it
/// names repairs nothing, which is why folding the two into one number would
/// cost the reader the only thing that tells them apart.
///
/// A standby row nobody dialled is `unverified` and is exempt by the same
/// rule. A standby that answered from its own host is `standby_serving`: a
/// second copy beside the active host, counted as the third failure.
pub(in crate::cli::service_verify) fn fail_on_unreachable(
    findings: &[Finding],
) -> Result<(), CmdError> {
    let count = |state: &str| {
        findings
            .iter()
            .filter(|finding| finding.state == state)
            .count()
    };
    let broken = count(UNREACHABLE);
    let misowned = count(MISOWNED);
    let second_copies = count(STANDBY_SERVING);
    if broken == 0 && misowned == 0 && second_copies == 0 {
        return Ok(());
    }
    if second_copies > 0 {
        eprintln!(
            "{second_copies} standby host(s) are serving a service that is active elsewhere; \
             each keeps its own copy of the service's state beside the active host's"
        );
    }
    if broken > 0 {
        eprintln!(
            "{broken} declaration(s) point at an endpoint that answered nothing from the host \
             that is told to call it"
        );
    }
    if misowned > 0 {
        eprintln!(
            "{misowned} declaration(s) name a port a different declared unit is holding; the \
             declaration is wrong, not the service it names"
        );
    }
    Err(CmdError::silent(1))
}
