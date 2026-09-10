//! Binding one immutable coordinate to exactly one source revision, before
//! any artifact byte is written into it.

use serde_json::json;

use crate::cli::CmdError;
use crate::release_control;

use super::ReleaseClaimCoordinateArgs;

/// What claiming a coordinate found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoordinateClaim {
    /// This publisher wrote the coordinate's revision record.
    Claimed,
    /// The record already existed and names this same build.
    Confirmed,
}

impl CoordinateClaim {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Claimed => "claimed",
            Self::Confirmed => "confirmed",
        }
    }
}

/// A refusal this product decided, not an unattributable failure.
///
/// One version can only ever mean one build, and a publisher that arrives
/// with a second source revision is refused by that rule. Reported as
/// `unknown`, the same refusal used to read as "your request or credentials",
/// which sends a release operator to inspect a credential for a decision the
/// release channel made on its own declared terms.
fn refused(message: String) -> CmdError {
    CmdError::click(message).stating(crate::failure::FailureCode::Refused)
}

/// Bind one immutable version and its platform coordinate to exactly one source
/// revision before any artifact is written.
///
/// The version-scoped record is the arbitration point shared by every
/// publisher and every platform. Its create-only write closes the race where
/// two publishers each observed no sibling claim and then claimed opposite
/// platforms from different commits.
pub(crate) async fn claim_release_coordinate(
    product: &str,
    version: &str,
    platform: &str,
    source_revision: &str,
) -> Result<CoordinateClaim, CmdError> {
    let version_claim = release_control::VersionRevision::new(product, version, source_revision)
        .map_err(CmdError::click)?;
    claim_release_version(&version_claim).await?;

    let claim =
        release_control::CoordinateRevision::new(product, version, platform, source_revision)
            .map_err(CmdError::click)?;
    let bytes = claim.canonical_bytes().map_err(CmdError::click)?;
    let base =
        release_control::release_base(product, version, platform).map_err(CmdError::click)?;
    let uri = format!("{base}/{}", release_control::RELEASE_REVISION_NAME);
    if let Ok(existing) = crate::cli::storage::fetch_object_from_writer(&uri).await {
        return judge_existing_claim(&claim, &existing, &uri);
    }
    let temporary = tempfile::NamedTempFile::new()?;
    std::fs::write(temporary.path(), &bytes)?;
    match crate::cli::storage::store_object(
        &uri,
        &temporary.path().display().to_string(),
        "application/json",
        true,
    )
    .await
    {
        Ok(_) => Ok(CoordinateClaim::Claimed),
        Err(error) => match crate::cli::storage::fetch_object_from_writer(&uri).await {
            Ok(existing) => judge_existing_claim(&claim, &existing, &uri),
            Err(_) => Err(error),
        },
    }
}

async fn claim_release_version(
    claim: &release_control::VersionRevision,
) -> Result<CoordinateClaim, CmdError> {
    let base = release_control::release_version_base(&claim.product, &claim.version)
        .map_err(CmdError::click)?;
    let uri = format!("{base}/{}", release_control::RELEASE_VERSION_REVISION_NAME);
    if let Ok(existing) = crate::cli::storage::fetch_object_from_writer(&uri).await {
        return judge_existing_version_claim(claim, &existing, &uri);
    }

    require_existing_platform_claims_agree(claim).await?;
    let bytes = claim.canonical_bytes().map_err(CmdError::click)?;
    let temporary = tempfile::NamedTempFile::new()?;
    std::fs::write(temporary.path(), &bytes)?;
    match crate::cli::storage::store_object(
        &uri,
        &temporary.path().display().to_string(),
        "application/json",
        true,
    )
    .await
    {
        Ok(_) => Ok(CoordinateClaim::Claimed),
        Err(error) => match crate::cli::storage::fetch_object_from_writer(&uri).await {
            Ok(existing) => judge_existing_version_claim(claim, &existing, &uri),
            Err(_) => Err(error),
        },
    }
}

/// Backfill a version-scoped claim only when every platform already present
/// carries a readable claim for this exact source revision.
async fn require_existing_platform_claims_agree(
    claim: &release_control::VersionRevision,
) -> Result<(), CmdError> {
    let coordinates = crate::cli::storage::published_release_coordinates(&claim.product).await?;
    for coordinate in coordinates {
        if coordinate.version != claim.version {
            continue;
        }
        if coordinate.version_scope {
            if coordinate.claim_only() {
                continue;
            }
            return Err(CmdError::click(format!(
                "cannot backfill the version claim because releases/{}/{} contains unexpected version-scoped objects: {}",
                claim.product,
                claim.version,
                coordinate.names.iter().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        let base =
            release_control::release_base(&claim.product, &claim.version, &coordinate.platform)
                .map_err(CmdError::click)?;
        let uri = format!("{base}/{}", release_control::RELEASE_REVISION_NAME);
        let bytes = crate::cli::storage::fetch_object_from_writer(&uri)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "cannot backfill the version claim because {uri} is unreadable: {error}"
                ))
            })?;
        let held: release_control::CoordinateRevision =
            serde_json::from_slice(&bytes).map_err(|error| {
                CmdError::click(format!(
                    "cannot backfill the version claim because {uri} is invalid: {error}"
                ))
            })?;
        if !held.describes(&claim.product, &claim.version, &coordinate.platform)
            || held.source_revision != claim.source_revision
        {
            return Err(refused(format!(
                "{}/{} already carries platform {} from source revision {}; this publisher \
                 carries {}. A version's platforms are one build: publish a new version",
                claim.product,
                claim.version,
                coordinate.platform,
                held.source_revision,
                claim.source_revision
            )));
        }
    }
    Ok(())
}

fn judge_existing_version_claim(
    claim: &release_control::VersionRevision,
    existing: &[u8],
    uri: &str,
) -> Result<CoordinateClaim, CmdError> {
    let held: release_control::VersionRevision =
        serde_json::from_slice(existing).map_err(|error| {
            CmdError::click(format!("{uri} is not a version revision record: {error}"))
        })?;
    if !held.describes(&claim.product, &claim.version) {
        return Err(refused(format!(
            "{uri} attests {}/{} and not {}/{}",
            held.product, held.version, claim.product, claim.version
        )));
    }
    if held.source_revision != claim.source_revision {
        return Err(refused(format!(
            "{}/{} already attests source revision {}, and this publisher carries {}. \
             Release objects are immutable: publish a new version",
            claim.product, claim.version, held.source_revision, claim.source_revision
        )));
    }
    Ok(CoordinateClaim::Confirmed)
}

fn judge_existing_claim(
    claim: &release_control::CoordinateRevision,
    existing: &[u8],
    uri: &str,
) -> Result<CoordinateClaim, CmdError> {
    let held: release_control::CoordinateRevision =
        serde_json::from_slice(existing).map_err(|error| {
            CmdError::click(format!(
                "{uri} is not a coordinate revision record: {error}"
            ))
        })?;
    if !held.describes(&claim.product, &claim.version, &claim.platform) {
        return Err(refused(format!(
            "{uri} attests {}/{}/{} and not {}/{}/{}",
            held.product, held.version, held.platform, claim.product, claim.version, claim.platform
        )));
    }
    if held.source_revision != claim.source_revision {
        return Err(refused(format!(
            "{}/{}/{} already attests source revision {}, and this publisher carries {}. \
             Release objects are immutable, so one version can never mean two builds: publish a \
             new version instead of writing a second build into this coordinate",
            claim.product,
            claim.version,
            claim.platform,
            held.source_revision,
            claim.source_revision
        )));
    }
    Ok(CoordinateClaim::Confirmed)
}

/// Claim an immutable release coordinate before publication.
pub(in crate::cli::release_cmd) async fn claim_coordinate(
    args: &ReleaseClaimCoordinateArgs,
) -> Result<(), CmdError> {
    let outcome = claim_release_coordinate(
        &args.product,
        &args.version,
        &args.platform,
        &args.source_commit,
    )
    .await
    .map_err(|error| error.machine_readable(args.json))?;
    let base = release_control::release_base(&args.product, &args.version, &args.platform)
        .map_err(CmdError::click)?;
    let uri = format!("{base}/{}", release_control::RELEASE_REVISION_NAME);
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "state": outcome.label(),
                "uri": uri,
                "product": args.product,
                "version": args.version,
                "platform": args.platform,
                "source_revision": args.source_commit,
            }))?
        );
    } else {
        println!(
            "{} {} {} {} at source revision {}",
            outcome.label(),
            args.product,
            args.version,
            args.platform,
            args.source_commit
        );
    }
    Ok(())
}
