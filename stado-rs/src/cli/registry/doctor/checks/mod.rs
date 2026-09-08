//! The checks `registry doctor` runs: the document read against itself here,
//! the service directory against release control ([`directory`]), one target
//! against its beacon ([`target`]), and the symptom rows one cause makes
//! redundant ([`dedup`]).

pub(super) mod dedup;
pub(super) mod directory;
pub(super) mod target;

use serde_json::Value;

use crate::cli::registry::doctor::findings::Finding;
use crate::targets::Registry;

/// The build's own verdict on the document every resolver on the fleet reads.
pub(super) fn resolver_refusal(document: &Value, findings: &mut Vec<Finding>) {
    // A document the inference contract refuses is not a cosmetic fault: every
    // resolver on the fleet validates the same way before it adopts a
    // generation, so it keeps serving the last copy it accepted and hands
    // consumers an address the fleet has since moved away from. On 2026-09-06 a
    // route alias without a namespace ("wisent-backend") published that state:
    // the always-on host's resolver froze eleven generations back, every chat
    // took `connection refused` from a candidate port nothing served any more,
    // and the only trace was one line in that resolver's log.
    if let Err(error) = crate::inference::schema::validate(document) {
        findings.push(Finding::new(
            "resolver-refuses-registry",
            "registry",
            format!(
                "every resolver refuses this document and keeps serving the last \
                 generation it accepted, so consumers resolve to addresses this \
                 registry no longer declares: {error}"
            ),
        ));
    }
}

/// The release-control block every version and environment check below is
/// measured against.
pub(super) fn declared_release_control(
    registry: &Registry,
    findings: &mut Vec<Finding>,
) -> Option<crate::release_control::ReleaseControl> {
    // What the fleet declares DELIVERED, which is what a missing version
    // declaration is measured against. Read from the document's own
    // `release_control` block and never from a unit on the host: a product
    // stays a release target after its launchd plist is removed, and so does
    // the version gap. A block that will not parse is reported rather than
    // skipped — a check that quietly measures nothing is the defect it was
    // written to catch.
    match registry
        .extra
        .get(crate::release_control::RELEASE_CONTROL_KEY)
    {
        Some(value) => {
            match <crate::release_control::ReleaseControl as serde::Deserialize>::deserialize(value)
            {
                Ok(control) => Some(control),
                Err(error) => {
                    findings.push(Finding::new(
                        "unreadable-release-control",
                        "registry",
                        format!(
                            "registry.{} did not parse, so no delivered product was judged \
                             against its declared version: {error}",
                            crate::release_control::RELEASE_CONTROL_KEY
                        ),
                    ));
                    None
                }
            }
        }
        None => None,
    }
}
