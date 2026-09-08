//! The standing audit of what the release channel already holds.

use std::time::Duration;

use super::claims::{
    claim_only_verdict, in_flight, require_version_claim_agreement, CLAIM_WITHOUT_ARTIFACT,
};
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

/// This row's own wall clock, for the same reason [`FLEET_SHAPE_DEADLINE`] has
/// one: its work grows with the channel, so it cannot share the flat
/// [`PROBE_TIMEOUT`].
///
/// The arithmetic, not a guess. [`INTEGRITY_VERSIONS`] versions times two
/// platforms is twelve coordinates; each one reads its `SHA256SUMS` and then
/// probes the nine names that file declares. That is about 120 network reads,
/// and one `storage stat` against the release channel measured 1.5 to 5
/// seconds during the 0.13.46 publication. Under the 8-second flat budget the
/// row could not finish its first coordinate, so the standing audit of the
/// release channel -- the check whose entire purpose is to notice
/// `stado/0.10.0/darwin-arm64` sitting half-published for four months -- has
/// been answering `probe did not answer within 8s` instead of auditing
/// anything.
///
/// The nine probes per coordinate run concurrently, and so do the two manifest
/// reads that check the coordinate was built from one revision, which is what
/// makes this bound sufficient rather than merely generous: twelve coordinates
/// at two round trips each, not 144 in series.
pub(in crate::doctor) const INTEGRITY_DEADLINE: Duration = Duration::from_secs(180);

/// Walk what the channel actually holds and say, per version and platform,
/// whether the coordinate is deliverable: every object present, and every
/// publisher of it naming one build.
///
/// This exists because the only thing that ever audited the channel was the
/// act of publishing to it or delivering from it. `stado/0.10.0/darwin-arm64`
/// was half-published in April and found by accident in August, while hunting
/// something else; `stado/0.11.0/darwin-arm64` sat at 4 objects of 9 and
/// `stado/0.12.1/linux-amd64` at 2 of 9 for the same reason.
///
/// Two failures are reported and both are permanent, because release objects
/// are create-only. PARTIAL is a coordinate short of objects that can never be
/// added. The second is a coordinate whose two publishers built different
/// revisions, which no later publication can reconcile either — 0.13.27 on
/// 2026-09-01 and 0.13.49 on 2026-09-03, both discovered by a delivery attempt
/// long after the train had finished writing them.
///
/// A version with no claim and no artifacts has no keys in the store, so it
/// never appears in this walk. A publisher now claims the version before any
/// platform, so a crash at that boundary appears as a version-scoped claim
/// and is aged by the same publication budget as a platform-only legacy
/// claim.
///
/// Presence comes from [`crate::deploy::host_release::missing_release_objects`],
/// which reads the coordinate's own `SHA256SUMS` and probes every name it
/// declares through `storage stat`. Revision agreement comes from
/// [`crate::deploy::host_release::coordinate_revision_conflict`], the same
/// comparison `host release` refuses on. No second checksum parser, no second
/// binary list, no second definition of what one build means, and an
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
        // one, and the object audit below cannot tell the difference: the
        // claim carries no list of what a complete coordinate holds, so
        // `missing_release_objects` can only answer `absent: SHA256SUMS` —
        // the same sentence it gives a coordinate that published eight
        // objects of nine. `stado/0.14.4/darwin-arm64` read exactly that on
        // 2026-09-03 while holding one 144-byte object, and the number was
        // already spent.
        if coordinate.claim_only() {
            let (status, sentence) = claim_only_verdict(product, coordinate).await;
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
                    // here — archive, manifest and SHA256SUMS present, five
                    // binaries absent, permanently.
                    //
                    // Unless it is happening right now. A publisher writes
                    // its objects one at a time, so every release passes
                    // through "short of objects" on the way to whole, and the
                    // claim it wrote first says when that began. Reading
                    // 0.14.5 as PARTIAL at 18:23 while its train was still
                    // uploading is the same false alarm the claim-only branch
                    // above exists to avoid, and the same clock answers both.
                    if in_flight(coordinate) {
                        findings.note(
                            Status::Unmeasured,
                            format!(
                                "{version}/{platform} is publishing now: {} object(s) not written \
                                 yet ({}), within the {} minute budget its claim started",
                                missing.len(),
                                missing.join(", "),
                                CLAIM_WITHOUT_ARTIFACT.num_minutes()
                            ),
                        );
                        false
                    } else {
                        findings.note(
                            Status::Fail,
                            format!(
                                "{version}/{platform} is PARTIAL and permanently so; absent: {}",
                                missing.join(", ")
                            ),
                        );
                        false
                    }
                }
                Err(error) => {
                    findings.note(
                        Status::Warn,
                        format!("{version}/{platform} could not be audited: {error}"),
                    );
                    false
                }
            };
        if !complete {
            continue;
        }
        // Complete is not the same as coherent. Every object a coordinate
        // needs can be present while two of them were built from different
        // revisions, because two publishers write one coordinate and
        // create-only puts mean neither can overwrite the other. That
        // coordinate counts nine of nine here and is still undeliverable, so
        // counting only presence is what let 0.13.49 pass this row at 06:14
        // while `host release --dry-run` refused it at 06:09.
        match crate::deploy::host_release::coordinate_revision_conflict(product, version, platform)
            .await
        {
            Ok(None) => whole += 1,
            Ok(Some(conflict)) => findings.note(Status::Fail, conflict),
            Err(error) => findings.note(
                Status::Warn,
                format!("{version}/{platform} could not be audited for one build: {error}"),
            ),
        }
    }
    if whole == audited {
        findings.note(
            Status::Pass,
            format!(
                "{whole} of {audited} recent published coordinates are whole and built from one \
                 revision"
            ),
        );
    }
    findings.measure(crate::fleet_shape::Measurement::new(
        INTEGRITY_ID,
        None,
        audited as u64,
    ));
    findings.into_check(INTEGRITY_ID, INTEGRITY_TITLE, INTEGRITY_REMEDY)
}
