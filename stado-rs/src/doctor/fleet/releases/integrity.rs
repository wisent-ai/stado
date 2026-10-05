//! The standing audit of what the release channel already holds.

use super::claims::{claim_only_verdict, publishing_run, require_version_claim_agreement, Runs};
use crate::doctor::{Check, Findings, Status};

// ---------------------------------------------------------------------------
// 4b. Release channel integrity
// ---------------------------------------------------------------------------

pub(in crate::doctor) const INTEGRITY_ID: &str = "release_integrity";
pub(in crate::doctor) const INTEGRITY_TITLE: &str = "Published release integrity";
pub(in crate::doctor) const INTEGRITY_REMEDY: &str =
    "a PARTIAL coordinate can never be completed: release objects are \
     create-only, so publish a new version rather than re-running the train for that one. Run \
     `stado host release --dry-run` against any version before promoting it";

/// How many published versions back this walks. The channel holds every
/// version ever released, and an audit that re-reads all of them on every
/// `doctor` would be slow enough that someone turns it off.
const INTEGRITY_VERSIONS: usize = 6;
/// Walk what the channel actually holds and say, per version and platform,
/// whether the coordinate is deliverable: every object of the signed release
/// present.
///
/// This exists because the only thing that ever audited the channel was the
/// act of publishing to it or delivering from it. `stado/0.10.0/darwin-arm64`
/// was half-published in April and found by accident in August, while hunting
/// something else.
///
/// The failure is permanent, because release objects are create-only: PARTIAL
/// is a coordinate short of objects that can never be added.
///
/// A version with no claim and no artifacts has no keys in the store, so it
/// never appears in this walk. A publisher now claims the version before any
/// platform, so a crash at that boundary appears as a version-scoped claim
/// and is aged by the same publication budget as a platform-only legacy
/// claim.
///
/// Presence comes from [`crate::deploy::host_release::missing_release_objects`],
/// the same signed-release object set `host release` refuses on, and an
/// unreachable store propagates as an error instead of being counted as an
/// absent object.
pub(in crate::doctor) async fn check_release_integrity() -> Check {
    let mut findings = Findings::default();
    findings.remedy(INTEGRITY_REMEDY);

    let product = match crate::deploy::products::product("stado") {
        Ok(product) => product,
        Err(error) => {
            findings.note(Status::Fail, format!("stado is not declared: {error}"));
            return findings.into_check(INTEGRITY_ID, INTEGRITY_TITLE, INTEGRITY_REMEDY);
        }
    };
    let coordinates = match crate::cli::storage::published_release_coordinates("stado").await {
        Ok(coordinates) => coordinates,
        Err(error) => {
            findings.note(
                Status::Warn,
                format!("the release channel could not be listed, so nothing was audited: {error}"),
            );
            return findings.into_check(INTEGRITY_ID, INTEGRITY_TITLE, INTEGRITY_REMEDY);
        }
    };
    if coordinates.is_empty() {
        findings.note(
            Status::Warn,
            "the release channel holds no published coordinate".to_string(),
        );
        return findings.into_check(INTEGRITY_ID, INTEGRITY_TITLE, INTEGRITY_REMEDY);
    }

    let mut versions: Vec<String> = Vec::new();
    for coordinate in &coordinates {
        if !versions.contains(&coordinate.version) {
            versions.push(coordinate.version.clone());
        }
    }
    versions.truncate(INTEGRITY_VERSIONS);

    // The publisher's own run records say which coordinates are still being
    // written; read once for the whole walk.
    let runs: Runs = crate::cli::release_submit::recorded_runs()
        .await
        .map_err(|error| error.to_string());
    let mut whole = 0usize;
    let mut audited = 0usize;
    for coordinate in &coordinates {
        let (version, platform) = (&coordinate.version, &coordinate.platform);
        if !versions.contains(version) {
            continue;
        }
        audited += 1;
        if coordinate.version_scope && !coordinate.claim_only() {
            findings.note(
                Status::Fail,
                format!(
                    "{version} contains unexpected version-scoped objects: {}",
                    coordinate
                        .names
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
            continue;
        }
        if let Err(error) = require_version_claim_agreement(product, coordinate).await {
            findings.note(Status::Fail, error);
            continue;
        }
        // A coordinate holding its claim and nothing else is not a partial
        // one, and the object audit below cannot tell the difference: a
        // coordinate holding one tiny claim object reads as absent
        // everything, while its number is already spent.
        if coordinate.claim_only() {
            let (status, sentence) = claim_only_verdict(product, coordinate, &runs).await;
            findings.note(status, sentence);
            continue;
        }
        let complete =
            match crate::deploy::host_release::missing_release_objects(product, version, platform)
                .await
            {
                Ok(missing) if missing.is_empty() => true,
                Ok(missing) => {
                    // The coordinates come from the store's own listing, so a
                    // version that published nothing has no keys and never
                    // appears here at all — which is the right outcome,
                    // because an empty coordinate holds nothing to be
                    // inconsistent with and the same tag can still publish
                    // cleanly. A coordinate that appears and is short has
                    // bytes that can never be completed: release objects are
                    // create-only.
                    //
                    // `stado/0.11.0/darwin-arm64` is the shape being caught
                    // here — objects present, others absent, permanently.
                    //
                    // Unless it is happening right now. A publisher writes
                    // its objects one at a time, so every release passes
                    // through "short of objects" on the way to whole, and its
                    // run record says whether it is still writing. Reading
                    // 0.14.5 as PARTIAL at 18:23 while its train was still
                    // uploading is the same false alarm the claim-only branch
                    // above exists to avoid, and the same record answers both.
                    match publishing_run(&runs, "stado", version, None) {
                        Ok(Some(run)) => findings.note(
                            Status::Unmeasured,
                            format!(
                                "{version}/{platform} is publishing now: {} object(s) not written \
                                 yet ({}), and release run {} is still running",
                                missing.len(),
                                missing.join(", "),
                                run.run_id
                            ),
                        ),
                        Ok(None) => findings.note(
                            Status::Fail,
                            format!(
                                "{version}/{platform} is PARTIAL and permanently so; absent: {}",
                                missing.join(", ")
                            ),
                        ),
                        Err(unreadable) => findings.note(
                            Status::Unmeasured,
                            format!(
                                "{version}/{platform} is short of {} ({}), and the release run \
                                 records could not be read ({unreadable}), so whether it is \
                                 still publishing could not be decided",
                                missing.len(),
                                missing.join(", ")
                            ),
                        ),
                    }
                    false
                }
                Err(error) => {
                    findings.note(
                        Status::Warn,
                        format!("{version}/{platform} could not be audited: {error}"),
                    );
                    false
                }
            };
        if complete {
            whole += 1;
        }
    }
    if whole == audited {
        findings.note(
            Status::Pass,
            format!("{whole} of {audited} recent published coordinates are whole"),
        );
    }
    findings.measure(crate::fleet_shape::Measurement::new(
        INTEGRITY_ID,
        None,
        audited as u64,
    ));
    findings.into_check(INTEGRITY_ID, INTEGRITY_TITLE, INTEGRITY_REMEDY)
}
