//! Publishing one immutable release coordinate: its key material, its
//! source-revision claim, the artifact and signature objects it writes, and
//! the verification that reads them back.

use std::path::PathBuf;

use clap::Args;

pub(super) mod claims;
pub(super) mod publish;
pub(super) mod signing;
pub(super) mod verification;

#[derive(Args)]
pub struct ReleaseKeygenArgs {
    #[arg(long)]
    private_key: PathBuf,
    #[arg(long)]
    public_key: PathBuf,
    #[arg(long)]
    key_id: String,
}

/// `stado release claim-coordinate` — the publishers' shared first step.
///
/// Exposed as a command because two of the three publishers are workflow
/// steps: the tag train and the existing-release recovery run in bash, and a
/// rule re-implemented in bash is a second source of truth for the one thing
/// that decides whether a version means one build.
#[derive(Args)]
pub struct ReleaseClaimCoordinateArgs {
    pub product: String,
    pub version: String,
    pub platform: String,
    /// The exact commit these bytes were built from.
    #[arg(long)]
    source_commit: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleasePrepareArgs {
    pub product: String,
    pub version: String,
    pub platform: String,
    #[arg(long)]
    archive: PathBuf,
    #[arg(long)]
    source_revision: String,
    #[arg(long)]
    binary: String,
    #[arg(long)]
    launcher: String,
    #[arg(
        long,
        conflicts_with = "signing_key_file",
        required_unless_present = "signing_key_file"
    )]
    signing_key_item: Option<String>,
    #[arg(
        long,
        conflicts_with = "signing_key_item",
        required_unless_present = "signing_key_item"
    )]
    signing_key_file: Option<PathBuf>,
    #[arg(long)]
    key_id: String,
    #[arg(long)]
    source_sha256: String,
    #[arg(long)]
    pipeline_manifest_sha256: String,
    #[arg(long)]
    qualification: PathBuf,
    #[arg(long, default_value_t = 1)]
    config_schema: u64,
    #[arg(long, default_value_t = 1)]
    state_schema: u64,
    #[arg(long)]
    minimum_stado_version: String,
    #[arg(long = "rollback-compatible-with")]
    rollback_compatible_with: Vec<String>,
    #[arg(long, default_value = "unknown")]
    builder: String,
    #[arg(long)]
    json: bool,
}
