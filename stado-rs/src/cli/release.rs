//! `stado release` — operator surface for [`crate::release`]: ask what the next
//! version is instead of deciding it from memory while publishing.
//!
//! The published artifact is the evidence. Because the release channel is
//! immutable and its downloads are bearer-free, the currently published binary
//! can always be fetched and asked what it can do. That is what makes the
//! comparison mechanical rather than a recollection of what changed.
//!
//! NO Python original: nothing there ever decided a version.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::table::print as print_table;
use super::CmdError;
use crate::release::manifest::ReleaseManifest;
use crate::release::{decide, Change, Surface, Version};

#[derive(Subcommand)]
pub enum ReleaseCommands {
    /// Decide the next version by comparing a candidate build's commands against
    /// the published build's, and say which rule produced the answer.
    Next(NextArgs),
    /// Print one build's observable command surface.
    Surface(SurfaceArgs),
    /// Build, classify, checksum and publish a product declared by its
    /// `.stado-release.json`. The same procedure for every product, so none of it
    /// has to be reimplemented per repository.
    Publish(PublishArgs),
    /// Generate this product's `.stado-release.json` from what the repository
    /// already says about itself, so nobody writes release wiring by hand.
    Init(InitArgs),
}

#[derive(Args)]
pub struct PublishArgs {
    /// Version already on the channel, to classify this candidate against.
    #[arg(long)]
    against: Option<String>,
    /// Write the derived version into the declared version file and stop.
    #[arg(long)]
    bump: bool,
    /// Declare breakage the command surface cannot show.
    #[arg(long)]
    breaking: bool,
    /// Resolve and report, building and publishing nothing.
    #[arg(long)]
    dry_run: bool,
    /// Product root; defaults to the working directory.
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct InitArgs {
    /// Product prefix in the channel; defaults to the package name.
    #[arg(long)]
    product: Option<String>,
    /// Product root; defaults to the working directory.
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct NextArgs {
    /// The version currently published, as a major.minor.patch triple.
    #[arg(long)]
    current: String,
    /// Executable already published at --current.
    #[arg(long)]
    published: String,
    /// Executable being considered for release.
    #[arg(long)]
    candidate: String,
    /// Declare breakage the command list cannot show: a field dropped from a
    /// payload, a stored format changed, an exit code repurposed. This can only
    /// escalate the classification, never lower it.
    #[arg(long)]
    breaking: bool,
    /// Subcommand each executable answers with its command list.
    #[arg(long, default_value = "help")]
    surface_command: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct SurfaceArgs {
    /// Executable to interrogate.
    binary: String,
    #[arg(long, default_value = "help")]
    surface_command: String,
    #[arg(long)]
    json: bool,
}

pub async fn dispatch(command: ReleaseCommands) -> Result<(), CmdError> {
    match command {
        ReleaseCommands::Next(args) => next(&args),
        ReleaseCommands::Surface(args) => surface(&args),
        ReleaseCommands::Publish(args) => publish(&args).await,
        ReleaseCommands::Init(args) => init(&args),
    }
}

/// Generate the manifest from what the repository already declares.
///
/// A product joining the channel should not have to write release logic, and it
/// should not have to write release wiring either: the facts are already in its
/// package manifest. Only stacks this can read honestly are generated — inventing
/// a build command for a stack it does not recognise would produce a manifest that
/// looks complete and publishes the wrong bytes.
fn init(args: &InitArgs) -> Result<(), CmdError> {
    let root = match &args.root {
        Some(path) => path.clone(),
        None => std::env::current_dir()?,
    };
    let target = root.join(crate::release::manifest::MANIFEST_NAME);
    if target.exists() {
        return Err(CmdError::click(format!(
            "{} already exists; edit it rather than regenerating, so a deliberate \
             change is never silently replaced",
            target.display()
        )));
    }

    let cargo = root.join("Cargo.toml");
    if !cargo.is_file() {
        return Err(CmdError::click(format!(
            "no Cargo.toml in {}: this generator only reads stacks it can read \
             honestly. Declare the manifest by hand — product, version_file, build, \
             artifact, and optionally surface_command, release_uri_env, commit_env",
            root.display()
        )));
    }
    let body = std::fs::read_to_string(&cargo)
        .map_err(|err| CmdError::click(format!("{}: {err}", cargo.display())))?;
    let package = crate::release::manifest::first_toml_string(&body, "name")
        .ok_or_else(|| CmdError::click(format!("{} declares no package name", cargo.display())))?;
    let product = args.product.clone().unwrap_or_else(|| package.clone());
    let stamp_prefix = product.to_ascii_uppercase().replace('-', "_");

    let manifest = json!({
        "_comment": "Facts about this product's releases. The procedure - guards, \
                     classification, checksum, create-only upload - lives in \
                     `stado release publish`, so it is not reimplemented here and \
                     cannot drift from what other products do.",
        "product": product,
        "version_file": "Cargo.toml",
        "build": ["cargo", "build", "--release", "--quiet"],
        "artifact": format!("target/release/{package}"),
        "surface_command": "help",
        "release_uri_env": format!("{stamp_prefix}_RELEASE_URI"),
        "commit_env": format!("{stamp_prefix}_RELEASE_COMMIT"),
    });
    let rendered = format!("{}\n", serde_json::to_string_pretty(&manifest)?);
    std::fs::write(&target, &rendered)
        .map_err(|err| CmdError::click(format!("{}: {err}", target.display())))?;

    // A product absent from the publisher map can still publish to a local store,
    // but an authenticated write would have nothing to authorize. Better said now
    // than discovered during a release.
    let known = crate::config::ACTIVE_RELEASE_PUBLISHERS.contains(&product.as_str());
    if args.json {
        return echo_json(&json!({
            "written": target.display().to_string(),
            "product": product,
            "registered_publisher": known,
        }));
    }
    println!("wrote {}", target.display());
    print!("{rendered}");
    if !known {
        println!(
            "\nNote: {product:?} is not in the release publisher map. Local publishing \
             works, but an authenticated write through a remote origin would have no \
             grant to authorize it."
        );
    }
    println!("\nNext: `stado release publish --dry-run`, then `--against <published> --bump`.");
    Ok(())
}

/// Run a command in the product root and return its stdout, failing loudly.
fn run(root: &Path, program: &str, args: &[String]) -> Result<String, CmdError> {
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|err| CmdError::click(format!("{program}: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CmdError::click(format!(
            "{program} {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git(root: &Path, args: &[&str]) -> Result<String, CmdError> {
    let owned: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    run(root, "git", &owned)
}

/// Refuse the two states that would bind an immutable coordinate to something
/// nobody can rebuild: a working copy, and a revision that lives on one machine.
fn revision(root: &Path) -> Result<String, CmdError> {
    if !git(root, &["status", "--porcelain"])?.is_empty() {
        return Err(CmdError::click(
            "the tree has uncommitted changes: commit them, so this version resolves \
             to a revision that can be rebuilt"
                .to_string(),
        ));
    }
    let ancestry = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
        .current_dir(root)
        .status()
        .map_err(|err| CmdError::click(format!("git: {err}")))?;
    if !ancestry.success() {
        return Err(CmdError::click(
            "HEAD is not on origin/main: push it first, or fetch if that ref is stale".to_string(),
        ));
    }
    git(root, &["rev-parse", "HEAD"])
}

async fn publish(args: &PublishArgs) -> Result<(), CmdError> {
    let root = match &args.root {
        Some(path) => path.clone(),
        None => std::env::current_dir()?,
    };
    let manifest = ReleaseManifest::load(&root).map_err(|err| CmdError::click(err.to_string()))?;
    let (program, build_args) = manifest
        .build
        .split_first()
        .ok_or_else(|| CmdError::click("the declared build command is empty"))?;
    let platform = crate::config::stado_release_platform();
    if platform.is_empty() {
        return Err(CmdError::click(
            "STADO_RELEASE_PLATFORM is unset: the platform is configuration, not a \
             guess this command is entitled to make"
                .to_string(),
        ));
    }
    let version = manifest
        .read_version(&root)
        .map_err(|err| CmdError::click(err.to_string()))?;
    let commit = revision(&root)?;

    let prefix = format!("stado://releases/{}/{version}/{platform}", manifest.product);
    let artifact_name = Path::new(&manifest.artifact)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CmdError::click("the declared artifact has no file name"))?
        .to_string();
    let binary_uri = format!("{prefix}/{artifact_name}");
    let sums_uri = format!("{prefix}/{}", crate::self_update::SHA256SUMS_NAME);

    if args.dry_run {
        return report_plan(
            args,
            &manifest,
            &version,
            &platform,
            &commit,
            &binary_uri,
            &sums_uri,
        );
    }

    // Bake the coordinate and the revision in, so the artifact names itself.
    let mut build = std::process::Command::new(program);
    build.args(build_args).current_dir(&root);
    if let Some(key) = &manifest.release_uri_env {
        build.env(key, &binary_uri);
    }
    if let Some(key) = &manifest.commit_env {
        build.env(key, &commit);
    }
    let built = build
        .status()
        .map_err(|err| CmdError::click(format!("{program}: {err}")))?;
    if !built.success() {
        return Err(CmdError::click("the declared build command failed"));
    }

    let artifact = root.join(&manifest.artifact);
    let bytes = std::fs::read(&artifact)
        .map_err(|err| CmdError::click(format!("{}: {err}", artifact.display())))?;
    let digest = hex::encode(Sha256::digest(&bytes));

    // Classify before uploading, so a wrong number is refused rather than burned
    // into a coordinate that can never be rewritten.
    let mut change = None;
    if let Some(against) = &args.against {
        let current = Version::parse(against).map_err(|err| CmdError::click(err.to_string()))?;
        let surface_command = manifest.surface_command.as_deref().ok_or_else(|| {
            CmdError::click(
                "this product declares no surface_command, so a change cannot be \
                 classified from evidence",
            )
        })?;
        let published_uri = format!(
            "stado://releases/{}/{against}/{platform}/{artifact_name}",
            manifest.product
        );
        let previous = super::storage::fetch_object(&published_uri).await?;
        let staged = tempfile::NamedTempFile::new()?;
        std::fs::write(staged.path(), &previous)?;
        // Executability is copied from the candidate rather than written as a mode,
        // so the permission is whatever this product's build already produces.
        std::fs::set_permissions(staged.path(), std::fs::metadata(&artifact)?.permissions())?;
        let published_surface = read_surface(&staged.path().to_string_lossy(), surface_command)?;
        let candidate_surface = read_surface(&artifact.to_string_lossy(), surface_command)?;
        let decision = decide(
            current,
            &published_surface,
            &candidate_surface,
            args.breaking,
        );
        let derived = decision.next.to_string();
        if args.bump {
            if derived == version {
                println!(
                    "{} already says {derived}; nothing to bump",
                    manifest.version_file
                );
                return Ok(());
            }
            manifest
                .write_version(&root, &derived)
                .map_err(|err| CmdError::click(err.to_string()))?;
            println!("change:  {} against {against}", decision.change.as_str());
            println!("{}: {version} -> {derived}", manifest.version_file);
            println!(
                "\ncommit and push that, then publish. The bump is a source change and is \
                 committed like any other, because a published coordinate has to resolve \
                 to a revision that is already pushed."
            );
            return Ok(());
        }
        if derived != version {
            return Err(CmdError::click(format!(
                "the surface change against {against} requires {derived}, but {} says \
                 {version}; derive it with --bump, or declare hidden breakage with --breaking",
                manifest.version_file
            )));
        }
        change = Some(decision.change);
    } else if args.bump {
        return Err(CmdError::click(
            "--bump needs --against: the number is derived from a comparison, so there is \
             nothing to derive it from without a predecessor"
                .to_string(),
        ));
    }

    // Refuse an artifact that cannot report where it came from.
    if manifest.release_uri_env.is_some() {
        verify_stamp(&artifact, &binary_uri, &commit)?;
    }

    let staged_sums = tempfile::NamedTempFile::new()?;
    std::fs::write(
        staged_sums.path(),
        format!("{digest}  {artifact_name}\n").as_bytes(),
    )?;

    super::storage::store_object(
        &binary_uri,
        &artifact.to_string_lossy(),
        "application/octet-stream",
        true,
    )
    .await?;
    super::storage::store_object(
        &sums_uri,
        &staged_sums.path().to_string_lossy(),
        "text/plain",
        true,
    )
    .await?;

    if args.json {
        return echo_json(&json!({
            "product": manifest.product,
            "version": version,
            "platform": platform,
            "commit": commit,
            "binary": binary_uri,
            "manifest": sums_uri,
            "digest": digest,
            "change": change.map(Change::as_str),
        }));
    }
    println!("published {} {version} for {platform}", manifest.product);
    println!("  {binary_uri}");
    println!("  {sums_uri}");
    Ok(())
}

/// Ask the built artifact what it is, and refuse to publish if it disagrees with
/// the coordinate it was built for.
fn verify_stamp(artifact: &Path, binary_uri: &str, commit: &str) -> Result<(), CmdError> {
    let output = std::process::Command::new(artifact)
        .arg("version")
        .output()
        .map_err(|err| CmdError::click(format!("{}: {err}", artifact.display())))?;
    let body = String::from_utf8_lossy(&output.stdout);
    let reported: Value = serde_json::from_str(&body).map_err(|err| {
        CmdError::click(format!(
            "{} version did not answer with JSON: {err}",
            artifact.display()
        ))
    })?;
    let field = |name: &str| {
        reported
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    if field("release") != binary_uri {
        return Err(CmdError::click(format!(
            "the build reports release {:?}, expected {binary_uri:?}",
            field("release")
        )));
    }
    if field("commit") != commit {
        return Err(CmdError::click(format!(
            "the build reports commit {:?}, expected {commit:?}",
            field("commit")
        )));
    }
    Ok(())
}

fn report_plan(
    args: &PublishArgs,
    manifest: &ReleaseManifest,
    version: &str,
    platform: &str,
    commit: &str,
    binary_uri: &str,
    sums_uri: &str,
) -> Result<(), CmdError> {
    if args.json {
        return echo_json(&json!({
            "product": manifest.product,
            "version": version,
            "platform": platform,
            "commit": commit,
            "binary": binary_uri,
            "manifest": sums_uri,
            "state": "dry run",
        }));
    }
    print_table(
        &["FIELD", "VALUE"],
        &[
            vec!["product".to_string(), manifest.product.clone()],
            vec!["version".to_string(), version.to_string()],
            vec!["platform".to_string(), platform.to_string()],
            vec!["commit".to_string(), commit.to_string()],
            vec!["binary".to_string(), binary_uri.to_string()],
            vec!["manifest".to_string(), sums_uri.to_string()],
        ],
    );
    println!("\ndry run — nothing built, nothing published.");
    if args.against.is_some() {
        println!("Classification needs a built candidate, so it is not part of a dry run.");
    }
    Ok(())
}

/// Ask a build what it can do. Run rather than read out of a source tree, because
/// the artifact is the thing being released and a checkout is not.
fn read_surface(binary: &str, surface_command: &str) -> Result<Surface, CmdError> {
    let output = std::process::Command::new(binary)
        .arg(surface_command)
        .output()
        .map_err(|err| CmdError::click(format!("{binary} {surface_command}: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CmdError::click(format!(
            "{binary} {surface_command} failed: {}",
            stderr.trim()
        )));
    }
    let body = String::from_utf8_lossy(&output.stdout);
    Surface::from_help(&body)
        .map_err(|err| CmdError::click(format!("{binary} {surface_command}: {err}")))
}

fn next(args: &NextArgs) -> Result<(), CmdError> {
    let current = Version::parse(&args.current).map_err(|err| CmdError::click(err.to_string()))?;
    let published = read_surface(&args.published, &args.surface_command)?;
    let candidate = read_surface(&args.candidate, &args.surface_command)?;
    let decision = decide(current, &published, &candidate, args.breaking);

    if args.json {
        return echo_json(&json!({
            "current": decision.current.to_string(),
            "next": decision.next.to_string(),
            "change": decision.change.as_str(),
            "added": decision.diff.added,
            "removed": decision.diff.removed,
            "declared_breaking": args.breaking,
            "unstable": decision.current.is_unstable(),
        }));
    }

    let mut rows = vec![
        vec!["current".to_string(), decision.current.to_string()],
        vec!["change".to_string(), decision.change.as_str().to_string()],
        vec!["next".to_string(), decision.next.to_string()],
    ];
    if !decision.diff.removed.is_empty() {
        rows.push(vec![
            "removed".to_string(),
            decision.diff.removed.join(", "),
        ]);
    }
    if !decision.diff.added.is_empty() {
        rows.push(vec!["added".to_string(), decision.diff.added.join(", ")]);
    }
    if args.breaking {
        rows.push(vec![
            "declared".to_string(),
            "breaking, by the operator".to_string(),
        ]);
    }
    print_table(&["FIELD", "VALUE"], &rows);

    // Name the rule that produced the number, so the answer can be argued with
    // instead of taken on faith.
    let reason = match (decision.change, decision.current.is_unstable()) {
        (Change::Breaking, true) => {
            "a removed or redefined contract, and under 0.x the minor slot carries compatibility"
        }
        (Change::Breaking, false) => "a removed or redefined contract",
        (Change::Additive, true) => {
            "added commands only, which under 0.x is a compatible change, so it lands in patch"
        }
        (Change::Additive, false) => "added commands only",
        (Change::Internal, _) => "an identical command surface",
    };
    println!("\n{reason}.");
    Ok(())
}

fn surface(args: &SurfaceArgs) -> Result<(), CmdError> {
    let surface = read_surface(&args.binary, &args.surface_command)?;
    if args.json {
        return echo_json(&json!({
            "binary": args.binary,
            "commands": surface.commands,
        }));
    }
    for command in &surface.commands {
        println!("{command}");
    }
    Ok(())
}

/// Same shape as `cli/storage.rs::echo_json`.
fn echo_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
