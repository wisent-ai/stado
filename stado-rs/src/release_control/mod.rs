//! Signed immutable product releases and registry-owned rollout policy.
//!
//! A release is built once, addressed by exact product/version/platform
//! coordinates, and signed over a canonical manifest. Promotion changes only
//! registry desired state; it never rebuilds or rewrites candidate bytes.
//!
//! One concern per component: `documents` is what a publication is made of,
//! `validation` is what those documents must say, `publisher` is the boundary
//! that signs and names them, and `install` is the immutable directory a
//! verified archive lands in. Everything the rest of the crate reads is
//! re-exported here, so `crate::release_control::<name>` stays the one
//! spelling of every name this module publishes.

mod documents;
mod install;
mod publisher;
mod validation;

pub use documents::manifest::{QualificationStatus, ReleaseManifest, ReleaseQualification};
pub use documents::policy::{
    BlueGreenServing, DesiredRelease, ProductReleasePolicy, ReleaseArtifactRef, ReleaseChannel,
    ReleaseControl, ReleaseTargetPolicy, RolloutStrategy, StrategyKind,
    DEFAULT_REPLACE_READINESS_PATH,
};
pub use documents::revision::{CoordinateRevision, VersionRevision};
pub use install::{
    install_directory, install_root_path, release_directory, safe_extract_archive,
    safe_extract_archive_file, safe_extract_source_archive,
};
pub use publisher::{
    canonical_manifest, control, generate_signing_key, release_base, release_version_base,
    sha256_bytes, sha256_file, sign_manifest, signing_public_key, verify_manifest,
};
pub use validation::manifest::validate_manifest;
pub use validation::registry::validate_registry_contract;
pub(crate) use validation::shape::{identifier, safe_absolute};

pub const RELEASE_CONTROL_KEY: &str = "release_control";
pub const RELEASE_MANIFEST_NAME: &str = "release.json";
pub const RELEASE_SIGNATURE_NAME: &str = "release.sig";
pub const RELEASE_ARCHIVE_NAME: &str = "release.tar.gz";
pub const RELEASE_QUALIFICATION_NAME: &str = "qualification.json";
/// The one object that says which build a coordinate belongs to.
///
/// Immutability protects an object, not a version. Two publishers write this
/// prefix — the tag train writes the executables, `SHA256SUMS`, the platform
/// archive and `release-manifest-<platform>.json`; the signed pipeline writes
/// [`RELEASE_MANIFEST_NAME`], [`RELEASE_SIGNATURE_NAME`],
/// [`RELEASE_ARCHIVE_NAME`] and [`RELEASE_QUALIFICATION_NAME`] — and those two
/// name sets are DISJOINT, so `--if-absent` never refused either of them.
/// A version number lives in `Cargo.toml`, which many commits share, so both
/// producers were entitled to the same coordinate from different revisions.
///
/// `stado/0.13.46/darwin-arm64` is what that costs: `release.json` attests
/// `446ad490…`, `release-manifest-darwin-arm64.json` attests `641a52b2…`, and
/// `pipeline_catalog_identity` refuses to deliver a version that means two
/// builds. It refuses at delivery, after both writes; immutable objects mean
/// the version can never be made to mean one build again.
///
/// This object is claimed create-only BEFORE any artifact, by every publisher,
/// so the second revision is refused while nothing has been written yet.
pub const RELEASE_REVISION_NAME: &str = "source-revision.json";
/// The one source claim shared by every platform of a product version.
pub const RELEASE_VERSION_REVISION_NAME: &str = "source-revision.json";

const MAX_RELEASE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Entries one release archive may carry. A release is a handful of
/// executables and their manifests, so a payload past this is not a release.
const MAX_ARCHIVE_ENTRIES: usize = 4096;
/// Entries one source snapshot may carry. A snapshot is a whole repository -
/// `git archive` of `wisent-ai/stado` listed 4,230 entries at `07fd9ba7`,
/// files and directories together - and the build worker used to extract it
/// under [`MAX_ARCHIVE_ENTRIES`], the bound sized for a release payload. On
/// 2026-09-10 the 0.20.9 build was refused on every builder with `release
/// archive exceeds 4096 entries` the moment the module split pushed the tree
/// over, and the coordinate stayed bound to a commit no installed worker
/// could unpack. The byte bounds still apply; only the count is sized for
/// what a repository is.
const MAX_SOURCE_ARCHIVE_ENTRIES: usize = 65_536;
