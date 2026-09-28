//! `stado release changelog-roll`: move the Unreleased entries a published
//! release already carries out of a checkout's `CHANGELOG.md` into
//! `changelog/<version>.md`.
//!
//! The header of every product changelog says a release moves its section
//! into `changelog/`, and nothing did: stado's own file sat past the 300-line
//! write limit with every entry since 0.16.40 under Unreleased while 0.22.13
//! was published. Which entries a release carries is a fact of git, not of
//! the text: an entry is released when the commit that added it is an
//! ancestor of the source commit the newest published run was built from.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Args;

use crate::cli::CmdError;

/// The line limit the workshop's write gate enforces on every file; a roll
/// whose released section would not fit one file is refused.
const FILE_LINE_LIMIT: usize = 300;

/// How many recent runs are searched for the newest published one.
const RUN_WINDOW: usize = 50;

/// Characters of an entry's first line used to find the commit that added it.
const NEEDLE_CHARS: usize = 80;

#[derive(Args)]
pub struct ReleaseChangelogRollArgs {
    /// Product whose newest published run decides what is released.
    pub product: String,
    /// The product's checkout holding CHANGELOG.md.
    #[arg(long, default_value = ".")]
    pub source: PathBuf,
    /// Print what would move without writing.
    #[arg(long)]
    pub plan: bool,
}

pub(in crate::cli::release_cmd) async fn changelog_roll(
    args: &ReleaseChangelogRollArgs,
) -> Result<(), CmdError> {
    let (version, commit) = newest_published(&args.product).await?;
    let path = args.source.join("CHANGELOG.md");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| CmdError::click(format!("{}: {error}", path.display())))?;
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == "## Unreleased")
        .ok_or_else(|| {
            CmdError::click(format!("{}: no '## Unreleased' section", path.display()))
        })?;
    let end = lines[start + 1..]
        .iter()
        .position(|line| line.starts_with("## "))
        .map_or(lines.len(), |offset| start + 1 + offset);
    let mut released = Vec::new();
    let mut kept = Vec::new();
    for entry in entries(&lines[start + 1..end]) {
        match added_by(&args.source, &entry)? {
            Some(added) if is_ancestor(&args.source, &added, &commit)? => released.push(entry),
            _ => kept.push(entry),
        }
    }
    println!(
        "{} {version} (source {commit}): {} released entries to move, {} stay under Unreleased",
        args.product,
        released.len(),
        kept.len()
    );
    if released.is_empty() || args.plan {
        return Ok(());
    }
    let (target, link) = write_released(&args.source, &version, &released)?;
    let mut out: Vec<String> = lines[..start].iter().map(|line| line.to_string()).collect();
    add_link(&mut out, &link);
    out.push("## Unreleased".to_string());
    out.push(String::new());
    for entry in &kept {
        out.push(entry.clone());
        out.push(String::new());
    }
    out.extend(lines[end..].iter().map(|line| line.to_string()));
    std::fs::write(&path, out.join("\n") + "\n")
        .map_err(|error| CmdError::click(format!("{}: {error}", path.display())))?;
    println!(
        "moved into {}; CHANGELOG.md now {} lines",
        target.display(),
        out.len()
    );
    Ok(())
}

/// The version and source commit of the newest run that published at least
/// one platform.
async fn newest_published(product: &str) -> Result<(String, String), CmdError> {
    let runs = crate::cli::release_submit::recent_runs(Some(product), RUN_WINDOW).await?;
    runs.iter()
        .find(|run| {
            run["platforms"].as_object().is_some_and(|platforms| {
                platforms
                    .values()
                    .any(|platform| platform["state"] == "published")
            })
        })
        .and_then(|run| {
            Some((
                run["version"].as_str()?.to_string(),
                run["source_commit"].as_str()?.to_string(),
            ))
        })
        .ok_or_else(|| {
            CmdError::click(format!(
                "{product}: none of the newest {RUN_WINDOW} release runs published a platform"
            ))
        })
}

/// Bullet entries of a section: each starts at a line beginning `- ` and runs
/// to the next such line; blank lines between entries are dropped.
fn entries(section: &[&str]) -> Vec<String> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    for line in section {
        if line.starts_with("- ") {
            out.push(vec![line]);
        } else if let Some(current) = out.last_mut() {
            current.push(line);
        }
    }
    out.into_iter()
        .map(|entry| entry.join("\n").trim_end().to_string())
        .collect()
}

fn git(source: &Path, args: &[&str]) -> Result<std::process::Output, CmdError> {
    Command::new("git")
        .arg("-C")
        .arg(source)
        .args(args)
        .output()
        .map_err(|error| CmdError::click(format!("git {}: {error}", args.join(" "))))
}

/// The oldest commit whose change to CHANGELOG.md added the entry's first
/// line, or `None` while the entry is not committed.
fn added_by(source: &Path, entry: &str) -> Result<Option<String>, CmdError> {
    let first = entry.lines().next().unwrap_or_default();
    let needle: String = first.chars().take(NEEDLE_CHARS).collect();
    let output = git(
        source,
        &["log", "--format=%H", "--reverse", "-S", &needle, "--", "CHANGELOG.md"],
    )?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::to_string))
}

fn is_ancestor(source: &Path, commit: &str, release: &str) -> Result<bool, CmdError> {
    let output = git(source, &["merge-base", "--is-ancestor", commit, release])?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(CmdError::click(format!(
            "git merge-base --is-ancestor {commit} {release}: {} (fetch the checkout so the \
             release's source commit is present)",
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

/// Write `changelog/<version>.md` with the released entries under a heading
/// for the version. Answers the file and the Released-list line naming it.
fn write_released(
    source: &Path,
    version: &str,
    released: &[String],
) -> Result<(PathBuf, String), CmdError> {
    let directory = source.join("changelog");
    std::fs::create_dir_all(&directory)
        .map_err(|error| CmdError::click(format!("{}: {error}", directory.display())))?;
    let target = directory.join(format!("{version}.md"));
    let mut body = vec![
        format!("# Changelog up to {version}"),
        String::new(),
        "Released entries, moved out of `CHANGELOG.md` by `stado release changelog-roll`."
            .to_string(),
        String::new(),
        format!("## {version}"),
        String::new(),
    ];
    body.extend(released.iter().cloned());
    let length: usize = body.iter().map(|line| line.lines().count().max(1)).sum();
    if length > FILE_LINE_LIMIT {
        return Err(CmdError::click(format!(
            "{version}: {length} released lines exceed one {FILE_LINE_LIMIT}-line file; roll \
             after each release"
        )));
    }
    std::fs::write(&target, body.join("\n") + "\n")
        .map_err(|error| CmdError::click(format!("{}: {error}", target.display())))?;
    Ok((target, format!("- [{version}](changelog/{version}.md)")))
}

/// Put the new file's link first in the `## Released` list.
fn add_link(out: &mut Vec<String>, link: &str) {
    if let Some(heading) = out.iter().position(|line| line.trim() == "## Released") {
        out.insert(heading + 2, link.to_string());
    }
}
