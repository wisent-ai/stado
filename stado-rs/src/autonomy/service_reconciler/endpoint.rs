//! The endpoint-side fact: what a reachability sweep proved about each
//! declared service's port.

use std::collections::BTreeMap;

use crate::observations::{OBSERVED, UNREACHABLE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EndpointState {
    Observed,
    Unreachable,
    Unverified,
    Absent,
}

impl EndpointState {
    pub(super) fn word(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Unreachable => "unreachable",
            Self::Unverified => "unverified",
            Self::Absent => "absent",
        }
    }
}

/// Fold the sweep's per-host rows into one state per service.
///
/// [`crate::observations::MISOWNED`] is mapped to
/// [`EndpointState::Unverified`] deliberately, in the arm below rather than
/// silently. It must never reach [`EndpointState::Unreachable`],
/// because that arm plans `ensure` -- a restart -- and a port held by another
/// declared unit is the one case where restarting the service named in the
/// declaration is guaranteed to fix nothing: the declaration is what is wrong,
/// the process it names is usually healthy on the port it actually serves, and
/// a reconciler cycling it every interval would turn a stale line in the
/// registry into a restart loop against a working service.
pub(super) fn endpoint_states(
    findings: &[crate::cli::service_verify::Finding],
) -> BTreeMap<String, EndpointState> {
    let mut states = BTreeMap::new();
    for finding in findings.iter().filter(|finding| finding.probed) {
        let next = match finding.state {
            OBSERVED => EndpointState::Observed,
            UNREACHABLE => EndpointState::Unreachable,
            // `misowned`, and any word a newer writer files, land here on
            // purpose. See this function's own note: the `Unreachable` arm
            // plans a restart, and this is the state where a restart cannot
            // help.
            _ => EndpointState::Unverified,
        };
        states
            .entry(finding.service.clone())
            .and_modify(|current| {
                *current = match (*current, next) {
                    (EndpointState::Observed, _) | (_, EndpointState::Observed) => {
                        EndpointState::Observed
                    }
                    (EndpointState::Unverified, _) | (_, EndpointState::Unverified) => {
                        EndpointState::Unverified
                    }
                    _ => EndpointState::Unreachable,
                }
            })
            .or_insert(next);
    }
    states
}
