//! What a release claim with nothing behind it means: a publication its
//! publisher is still running, or a version number permanently spent.

use crate::cli::release_submit::RecordedRun;
use crate::doctor::Status;

/// Every release pipeline run recorded for each `(product, version)`, as
/// [`crate::cli::release_submit::recorded_runs`] reads them, or why they
/// could not be read.
pub(super) type Runs =
    Result<std::collections::BTreeMap<(String, String), Vec<RecordedRun>>, String>;

/// The run still publishing this version, read from the release pipeline's
/// own run record rather than from the claim's age. `revision`, when the
/// claim was read, narrows it to the run that wrote that claim.
///
/// The publisher writes the claim immediately before its uploads, inside a
/// run whose record says whether it is still moving. A run that has not
/// finished is in flight however long its build waits for capacity; a run
/// that finished, or no run at all, means nothing will ever write the rest of
/// the coordinate. Unreadable run records answer `Err`: absent evidence is not
/// proof either way.
pub(super) fn publishing_run<'a>(
    runs: &'a Runs,
    product: &str,
    version: &str,
    revision: Option<&str>,
) -> Result<Option<&'a RecordedRun>, &'a str> {
    let runs = runs.as_ref().map_err(String::as_str)?;
    Ok(runs
        .get(&(product.to_string(), version.to_string()))
        .and_then(|recorded| {
            recorded.iter().find(|run| {
                revision.is_none_or(|revision| run.source_commit == revision)
                    && run.state.as_ref().is_some_and(|state| !state.finished())
            })
        }))
}

/// What a coordinate holding only its claim is: a publication in flight, or a
/// version number permanently spent.
///
/// The claim is create-only and so is every artifact, so a claim with nothing
/// behind it cannot be completed by a later run: the second publisher is
/// refused at the claim if it names another revision, and a rebuild of the
/// same revision can never reproduce byte-identical artifacts. The number is
/// gone, and the only remedy is a new version.
///
/// The claim's own body names the revision, which is the fact an operator
/// needs: it says which commit spent the number, and therefore whether the
/// version in `Cargo.toml` still has a run that can publish it.
pub(super) async fn claim_only_verdict(
    product: &crate::deploy::products::Product,
    coordinate: &crate::cli::storage::PublishedCoordinate,
    runs: &Runs,
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
    let claimed = match crate::cli::storage::fetch_object(&uri).await {
        Ok(bytes) if coordinate.version_scope => {
            serde_json::from_slice::<crate::release_control::VersionRevision>(&bytes)
                .map(|claim| claim.source_revision)
                .map_err(|error| format!("an unreadable claim record ({error})"))
        }
        Ok(bytes) => serde_json::from_slice::<crate::release_control::CoordinateRevision>(&bytes)
            .map(|claim| claim.source_revision)
            .map_err(|error| format!("an unreadable claim record ({error})")),
        Err(error) => Err(format!("a claim this audit could not read ({error})")),
    };
    let revision = match claimed {
        Ok(revision) => revision,
        Err(unreadable) => {
            // Without the revision no run can be matched to the claim, and
            // guessing either way is what this row exists to stop: calling a
            // live publication burnt is a false alarm on a release, calling a
            // spent number healthy is the silence itself.
            return (
                Status::Unmeasured,
                format!(
                    "{scope} holds only {unreadable}, so whether its publication is in flight \
                     or permanently spent could not be decided"
                ),
            );
        }
    };
    match publishing_run(
        runs,
        &product.source.product,
        version,
        Some(revision.as_str()),
    ) {
        Ok(Some(run)) => {
            let state = run
                .state
                .as_ref()
                .map(|state| state.phase())
                .unwrap_or_default();
            return (
                Status::Unmeasured,
                format!(
                    "{scope} is publishing now: its claim binds it to {revision}, and release \
                     run {} for that revision is still {state}",
                    run.run_id
                ),
            );
        }
        Ok(None) => {}
        Err(unreadable) => {
            return (
                Status::Unmeasured,
                format!(
                    "{scope} holds only its claim, bound to {revision}, and the release run \
                     records could not be read ({unreadable}), so whether its publication is in \
                     flight or permanently spent could not be decided"
                ),
            );
        }
    }
    (
        Status::Fail,
        format!(
            "{scope} is BURNT and permanently so: its claim binds it to {revision}, and no \
             release run for that revision is still publishing. Claim and artifacts are \
             create-only, so no later run can publish this version — the number is spent and a \
             publication of it must use a new one"
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
    Ok(())
}
