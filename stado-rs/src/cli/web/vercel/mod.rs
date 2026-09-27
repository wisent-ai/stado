//! `stado web vercel build` and `stado web vercel deploy`: a product hosted on
//! Vercel, built and delivered by the release pipeline through one Stado
//! command instead of a script per repository.
//!
//! The same two Python files — a prebuilt Vercel build and its production
//! deploy — were copied into wisent-trade, wisent-landing-new and weles-web,
//! with a third variant in wisent-app, each free to drift, after the workshop
//! had removed both Python and repository scripts. The build step runs on the
//! release worker inside the checkout Stado prepared and stages the prebuilt
//! output; the deploy step runs as the manifest's delivery against the
//! verified release archive.

mod deploy;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Subcommand;
use serde_json::json;

use crate::cli::CmdError;

use deploy::deploy;

/// The Vercel CLI the build and the deploy run, pinned so a prebuilt output
/// and the deploy that uploads it always come from one CLI release.
pub(super) const VERCEL_CLI: &str = "vercel@48.10.1";
/// The staged archive's name, as the manifest's stage map and the deploy
/// step both name it.
pub(super) const OUTPUT_ARCHIVE: &str = "vercel-output.tar.gz";
/// The shape of the build record and the delivery receipt, the one the
/// replaced scripts wrote, so their readers keep reading them.
pub(super) const RECORD_SCHEMA: u32 = 1;

#[derive(Debug, Subcommand)]
pub(crate) enum VercelCommands {
    /// Build the checked-out product with Vercel and stage the prebuilt output.
    ///
    /// Runs on a release worker. Reads VERCEL_TOKEN, VERCEL_ORG_ID and
    /// VERCEL_PROJECT_ID from the platform's secret_env; writes
    /// release/vercel-output.tar.gz and evidence/build.json under
    /// WISENT_OUTPUT_DIR.
    Build {
        /// Serve a private Git dependency from a release input instead of
        /// GitHub: INPUT=OWNER/REPOSITORY.git, where INPUT is the manifest's
        /// input name and the input is a Git bundle directory. Repeatable.
        #[arg(long = "git-input")]
        git_inputs: Vec<String>,
    },
    /// Deploy the verified release's prebuilt output to Vercel production.
    ///
    /// Runs as a manifest delivery. Refuses an archive whose digest differs
    /// from WISENT_RELEASE_SHA256, and an output holding a link, an absolute
    /// path or a parent segment; writes vercel-production-receipt.json.
    Deploy,
}

pub(crate) async fn dispatch(command: VercelCommands) -> Result<(), CmdError> {
    match command {
        VercelCommands::Build { git_inputs } => build(&git_inputs),
        VercelCommands::Deploy => deploy(),
    }
}

/// One variable of the worker or delivery contract, refused by name when absent.
pub(super) fn required(name: &str) -> Result<String, CmdError> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{name} is required and was not provided")))
}

/// Run one program with inherited output, stdin closed, refusing a failure by
/// its command line.
pub(super) fn run(command: &mut Command) -> Result<(), CmdError> {
    let rendered = format!("{command:?}");
    let status = command
        .stdin(Stdio::null())
        .status()
        .map_err(|error| CmdError::click(format!("cannot run {rendered}: {error}")))?;
    if !status.success() {
        return Err(CmdError::click(format!("{rendered} failed with {status}")));
    }
    Ok(())
}

fn build(git_inputs: &[String]) -> Result<(), CmdError> {
    let source = PathBuf::from(required("WISENT_SOURCE_DIR")?);
    let output = PathBuf::from(required("WISENT_OUTPUT_DIR")?);
    let product = required("WISENT_PRODUCT")?;
    let version = required("WISENT_VERSION")?;
    let platform = required("WISENT_PLATFORM")?;
    let token = required("VERCEL_TOKEN")?;
    let organisation = required("VERCEL_ORG_ID")?;
    let project = required("VERCEL_PROJECT_ID")?;

    let work = output.join("work");
    std::fs::create_dir_all(&work)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", work.display())))?;
    let git_config = work.join("gitconfig");
    for declared in git_inputs {
        rewrite_git_input(declared, &git_config)?;
    }

    let vercel = |arguments: &[&str]| {
        let mut command = Command::new("npx");
        command
            .args(["--yes", VERCEL_CLI])
            .args(arguments)
            .args(["--token", &token])
            .current_dir(&source)
            .env("VERCEL_ORG_ID", &organisation)
            .env("VERCEL_PROJECT_ID", &project)
            .env("GIT_CONFIG_GLOBAL", &git_config);
        command
    };
    run(&mut vercel(&["pull", "--yes", "--environment=production"]))?;
    run(&mut vercel(&["build", "--prod"]))?;

    let release = output.join("release");
    let evidence = output.join("evidence");
    for directory in [&release, &evidence] {
        std::fs::create_dir_all(directory).map_err(|error| {
            CmdError::click(format!("cannot create {}: {error}", directory.display()))
        })?;
    }
    let archive = release.join(OUTPUT_ARCHIVE);
    run(Command::new("tar")
        .args(["--dereference", "--format=ustar", "-czf"])
        .arg(&archive)
        .arg("-C")
        .arg(&source)
        .args([".vercel/output", ".vercel/project.json"])
        .env("COPYFILE_DISABLE", "true"))?;
    let (_, digest) = crate::release_control::sha256_file(&archive).map_err(CmdError::click)?;
    let record = json!({
        "schema_version": RECORD_SCHEMA,
        "product": product,
        "version": version,
        "platform": platform,
        "vercel_output_sha256": digest,
    });
    let written = evidence.join("build.json");
    std::fs::write(&written, format!("{record}\n"))
        .map_err(|error| CmdError::click(format!("cannot write {}: {error}", written.display())))?;
    println!("stado web vercel: staged {} ({digest})", archive.display());
    Ok(())
}

/// `INPUT=OWNER/REPOSITORY.git`: the input's bundle directory answers both
/// spellings GitHub is reached by, through the build's own Git config.
fn rewrite_git_input(declared: &str, git_config: &Path) -> Result<(), CmdError> {
    let (input, repository) = declared.split_once('=').ok_or_else(|| {
        CmdError::usage(format!(
            "--git-input takes INPUT=OWNER/REPOSITORY.git, not {declared:?}"
        ))
    })?;
    let variable = format!(
        "WISENT_INPUT_{}_BUNDLE_DIR",
        input.to_ascii_uppercase().replace('-', "_")
    );
    let bundle = required(&variable)?;
    let target = format!("url.file://{bundle}.insteadOf");
    let origins = [
        format!("ssh://git@github.com/{repository}"),
        format!("git@github.com:{repository}"),
    ];
    for (position, origin) in origins.iter().enumerate() {
        let mut command = Command::new("git");
        command
            .env("GIT_CONFIG_GLOBAL", git_config)
            .args(["config", "--global"]);
        if position > usize::MIN {
            command.arg("--add");
        }
        run(command.args([target.as_str(), origin.as_str()]))?;
    }
    Ok(())
}
