//! `stado build`: compile one committed source on every declared platform
//! and keep the result, without signing, publishing or promoting anything.
//!
//! A build is the first half of what `stado release submit` used to do in
//! one breath. It has its own durable record and its own identity — product,
//! version, source digest, manifest digest, no channel — so one build can be
//! read, retried and released more than once, and a release can be refused
//! for consuming a build that has not passed.

mod newest;
mod report;
mod submit;

use std::path::PathBuf;

use clap::{Args, Subcommand};
pub(crate) use report::current_build;
pub(crate) use submit::{
    ensure_build, ensure_object_store, read_source, record_build, stage_source,
};

/// A build id as `stado build status` prints it: the same 32 lowercase
/// hexadecimal characters a release run id has.
pub(crate) fn require_build_id(id: &str) -> Result<(), crate::cli::CmdError> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(crate::cli::CmdError::usage(
            "build ID must be 32 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum BuildCommands {
    /// Build one committed source on every platform its manifest declares.
    ///
    /// Snapshots the committed tree, publishes it as an immutable source
    /// object and queues one build job per platform on a live fleet builder.
    /// It ends when the jobs are queued; `stado build status` follows them.
    /// Nothing is signed, published or promoted: that is `stado release
    /// submit --build`, which consumes only a build that has passed.
    /// Repeating the command for the same source resumes the same build and
    /// re-queues only the platforms whose job failed or was cancelled.
    Submit(BuildSubmitArgs),
    /// Read one build: its source, and what each platform's job did.
    Status(BuildStatusArgs),
    /// List recent builds, newest first.
    List(BuildListArgs),
    /// Build every product in this workspace from the commit it stands on,
    /// and say what failed. Nothing is released.
    ///
    /// The same reading of the workspace as `stado release newest` — every
    /// product checkout, the commit it is on, the version that commit
    /// declares — followed by one build per product. `--plan` reads without
    /// queueing; `--wait` follows every build to its end and exits nonzero
    /// naming each product whose build failed.
    Newest(newest::BuildNewestArgs),
}

#[derive(Args)]
pub struct BuildSubmitArgs {
    /// The product checkout to build.
    #[arg(long)]
    pub source: PathBuf,
    /// Read this full Git commit without changing or requiring a clean checkout.
    #[arg(long)]
    pub commit: Option<String>,
    /// The version that commit declares in its version source.
    #[arg(long)]
    pub version: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct BuildStatusArgs {
    /// Full build ID from `stado build submit` or `stado build list`.
    pub build_id: String,
    /// Follow every platform's job to its end before answering, so the
    /// answer is `passed` or `failed`, never `waiting`.
    #[arg(long)]
    pub wait: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct BuildListArgs {
    /// One product; blank lists every product's builds.
    #[arg(long)]
    pub product: Option<String>,
    /// How many builds to list.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
}

pub async fn dispatch(command: BuildCommands) -> Result<(), crate::cli::CmdError> {
    match command {
        BuildCommands::Submit(args) => submit::submit(&args).await,
        BuildCommands::Status(args) => report::status(&args).await,
        BuildCommands::List(args) => report::list(&args).await,
        BuildCommands::Newest(args) => newest::newest(&args).await,
    }
}
