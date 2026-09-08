//! What a release claim with nothing behind it means, and how long it may
//! stand before the version number is called spent.

use chrono::Utc;
use serde_json::Value;

use crate::doctor::Status;

/// How long a claim may stand with no artifact behind it before the
/// coordinate is called burnt rather than in flight.
///
/// Both publishers write the claim immediately before their uploads, in the
/// same job: `deploy.yml` calls `release claim-coordinate` and then copies the
/// objects, in `publish-linux` for `linux-amd64` and in
/// `deploy-control-plane` for `darwin-arm64`. So the honest budget is one
/// publishing job's wall clock, not the whole train's — a train can wait
/// hours for release capacity, but it has not claimed anything while it
/// waits.
///
/// Measured, not guessed: in run 33693066772, the tag train for 0.13.49,
/// `publish-linux` took 21m29s (05:19:07 to 05:40:36) and
/// `deploy-control-plane` 20m08s (04:58:56 to 05:19:04), each of those
/// including the build that precedes the claim. Sixty minutes is roughly
/// three times the longest of them, so a slow but working publication is
/// never called burnt, while a claim older than that has outlived every
/// publication this channel has recorded.
///
/// It errs late on purpose. The cost of firing early is a false alarm on
/// every release, which is how a check gets turned off; the cost of firing
/// late is a report an hour after the number was already spent, and the
/// number stays spent either way. The train's own
/// `cancel-in-progress: true` concurrency group means a superseded
/// publication dies at once rather than finishing slowly, so most burnt
/// claims are already permanent long before this bound.
pub(super) const CLAIM_WITHOUT_ARTIFACT: chrono::Duration = chrono::Duration::minutes(60);

/// What a coordinate holding only its claim is: a publication in flight, or a
/// version number permanently spent.
///
/// The claim is create-only and so is every artifact, so a claim with nothing
/// behind it cannot be completed by a later train: the second publisher is
/// refused at the claim if it names another revision, and a rebuild of the
/// same revision can never reproduce byte-identical artifacts. The number is
/// gone, and the only remedy is a new version.
///
/// This is deliberately NOT part of
/// [`crate::deploy::host_release::coordinate_revision_conflict`], which
/// answers a different question about a different subject: that one compares
/// two publishers' manifests at a coordinate that HAS artifacts and reports a
/// coordinate built twice. This reports a coordinate built never. Folding
/// them together would make one sentence answer for both, and the remedies
/// are the same only by coincidence.
///
/// The claim's own body names the revision, which is the fact an operator
/// needs: it says which commit spent the number, and therefore whether the
/// version in `Cargo.toml` still has a train that can publish it.
pub(super) async fn claim_only_verdict(
    product: &crate::deploy::products::Product,
    coordinate: &crate::cli::storage::PublishedCoordinate,
) -> (Status, String) {
    let (version, platform) = (&coordinate.version, &coordinate.platform);
    let (scope, uri) = if coordinate.version_scope {
        (
            version.clone(),
            format!(
                "stado://releases/{}/{version}/{}",
                product.source.product,
                crate::release_control::RELEASE_VERSION_REVISION_NAME
            ),
        )
    } else {
        (
            format!("{version}/{platform}"),
            format!(
                "stado://releases/{}/{version}/{platform}/{}",
                product.source.product,
                crate::release_control::RELEASE_REVISION_NAME
            ),
        )
    };
    let revision = match crate::cli::storage::fetch_object(&uri).await {
        Ok(bytes) if coordinate.version_scope => {
            serde_json::from_slice::<crate::release_control::VersionRevision>(&bytes)
                .map(|claim| claim.source_revision)
                .unwrap_or_else(|error| format!("an unreadable claim record ({error})"))
        }
        Ok(bytes) => serde_json::from_slice::<crate::release_control::CoordinateRevision>(&bytes)
            .map(|claim| claim.source_revision)
            .unwrap_or_else(|error| format!("an unreadable claim record ({error})")),
        Err(error) => format!("a claim this audit could not read ({error})"),
    };
    let Some(written) = coordinate.claim_written_at else {
        // No timestamp means the age cannot be judged, and guessing either
        // way is what this row exists to stop: calling a live publication
        // burnt is a false alarm on a release, calling a spent number healthy
        // is the silence itself.
        return (
            Status::Unmeasured,
            format!(
                "{scope} holds only its claim, bound to {revision}, and the store \
                 reported no write time for it, so whether the publication is in flight or \
                 permanently spent could not be decided"
            ),
        );
    };
    let age = Utc::now() - written;
    let stamp = written.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    if age < CLAIM_WITHOUT_ARTIFACT {
        return (
            Status::Unmeasured,
            format!(
                "{scope} is publishing now: its claim was written {stamp}, binding \
                 it to {revision}, and no artifact has followed within the {} minute budget yet",
                CLAIM_WITHOUT_ARTIFACT.num_minutes()
            ),
        );
    }
    (
        Status::Fail,
        format!(
            "{scope} is BURNT and permanently so: its claim was written {stamp}, \
             binding it to {revision}, and no artifact followed within {} minutes. Claim and \
             artifacts are create-only, so no later train can publish this version — the number \
             is spent and a publication of it must use a new one",
            CLAIM_WITHOUT_ARTIFACT.num_minutes()
        ),
    )
}

pub(super) async fn require_version_claim_agreement(
    product: &crate::deploy::products::Product,
    coordinate: &crate::cli::storage::PublishedCoordinate,
) -> Result<(), String> {
    if coordinate.version_scope || !coordinate.has_version_claim {
        return Ok(());
    }
    let version_uri = format!(
        "stado://releases/{}/{}/{}",
        product.source.product,
        coordinate.version,
        crate::release_control::RELEASE_VERSION_REVISION_NAME
    );
    let platform_uri = format!(
        "stado://releases/{}/{}/{}/{}",
        product.source.product,
        coordinate.version,
        coordinate.platform,
        crate::release_control::RELEASE_REVISION_NAME
    );
    let version_bytes = crate::cli::storage::fetch_object(&version_uri)
        .await
        .map_err(|error| format!("cannot read version claim {version_uri}: {error}"))?;
    let platform_bytes = crate::cli::storage::fetch_object(&platform_uri)
        .await
        .map_err(|error| format!("cannot read platform claim {platform_uri}: {error}"))?;
    let version_claim: crate::release_control::VersionRevision =
        serde_json::from_slice(&version_bytes)
            .map_err(|error| format!("invalid version claim {version_uri}: {error}"))?;
    let platform_claim: crate::release_control::CoordinateRevision =
        serde_json::from_slice(&platform_bytes)
            .map_err(|error| format!("invalid platform claim {platform_uri}: {error}"))?;
    if !version_claim.describes(&product.source.product, &coordinate.version)
        || !platform_claim.describes(
            &product.source.product,
            &coordinate.version,
            &coordinate.platform,
        )
        || version_claim.source_revision != platform_claim.source_revision
    {
        return Err(format!(
            "{}/{} version claim and platform {} claim do not attest one source revision",
            product.source.product, coordinate.version, coordinate.platform
        ));
    }
    let base = format!(
        "stado://releases/{}/{}/{}",
        product.source.product, coordinate.version, coordinate.platform
    );
    let signed_uri = format!("{base}/{}", crate::release_control::RELEASE_MANIFEST_NAME);
    if crate::cli::storage::release_object_present(&signed_uri)
        .await
        .map_err(|error| error.to_string())?
    {
        let bytes = crate::cli::storage::fetch_object(&signed_uri)
            .await
            .map_err(|error| error.to_string())?;
        let manifest: crate::release_control::ReleaseManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid signed manifest {signed_uri}: {error}"))?;
        crate::release_control::validate_manifest(&manifest)?;
        if manifest.product != product.source.product
            || manifest.version != coordinate.version
            || manifest.platform != coordinate.platform
            || manifest.source_revision != version_claim.source_revision
        {
            return Err(format!(
                "{signed_uri} does not agree with the authoritative version claim"
            ));
        }
    }
    let sidecar_uri = format!("{base}/release-manifest-{}.json", coordinate.platform);
    if crate::cli::storage::release_object_present(&sidecar_uri)
        .await
        .map_err(|error| error.to_string())?
    {
        let bytes = crate::cli::storage::fetch_object(&sidecar_uri)
            .await
            .map_err(|error| error.to_string())?;
        let sidecar: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid platform manifest {sidecar_uri}: {error}"))?;
        if sidecar.get("product").and_then(Value::as_str) != Some(product.source.product.as_str())
            || sidecar.get("version").and_then(Value::as_str) != Some(coordinate.version.as_str())
            || sidecar.get("platform").and_then(Value::as_str) != Some(coordinate.platform.as_str())
            || sidecar.get("source_commit").and_then(Value::as_str)
                != Some(version_claim.source_revision.as_str())
        {
            return Err(format!(
                "{sidecar_uri} does not agree with the authoritative version claim"
            ));
        }
    }
    Ok(())
}

/// Whether this coordinate's publication is still inside the budget its claim
/// started.
///
/// One clock for both shapes of an unfinished publication — no artifacts at
/// all, and some artifacts — because they are the same event observed at
/// different moments, and two thresholds would eventually disagree about the
/// same release. A coordinate with no claim timestamp is not in flight: this
/// answers "provably still running", and absent evidence is not proof.
pub(super) fn in_flight(coordinate: &crate::cli::storage::PublishedCoordinate) -> bool {
    coordinate
        .claim_written_at
        .is_some_and(|written| Utc::now() - written < CLAIM_WITHOUT_ARTIFACT)
}
