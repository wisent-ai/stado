//! A program release a host installs from its publisher: the archive and the
//! checksum list the publisher puts beside it are fetched, the archive is
//! refused unless its sha256 is the one the list gives it, and it is unpacked
//! and placed as `<root>/<version>`. Node.js is installed this way, and the
//! Supabase CLI the schema release runs is installed the same
//! way, each from the release Stado declares for it, and each is taken as
//! installed only when it answers that release ([`answering`]).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use stado_wait as wait;

use crate::common::files::{present, remove_tree};
use crate::common::{capture, sha256, unpack};

/// `program` when it is installed and answers `version` to `--version`;
/// `None` when it is absent or answers another release, which an install
/// replaces. A program that is there and cannot be examined or run is the
/// error naming it.
pub fn answering(program: PathBuf, version: &str) -> Result<Option<PathBuf>> {
    if !present(&program)? {
        return Ok(None);
    }
    let output = wait::output(Command::new(&program).arg("--version"))
        .with_context(|| format!("running {} --version", program.display()))?;
    let answered = String::from_utf8_lossy(&output.stdout);
    Ok((output.status.success() && answered.trim() == version).then_some(program))
}

/// One archive of a release and the checksum list published beside it.
pub struct Published {
    /// The archive's file name, as the checksum list names it.
    pub archive_name: String,
    pub archive_url: String,
    /// The list of `<sha256>  <file name>` lines the release publishes.
    pub sums_url: String,
    /// The directory inside the archive that holds the release, or `None`
    /// when the archive holds its files at its root.
    pub tree: Option<String>,
}

fn fetch(url: &str, destination: &Path) -> Result<()> {
    let output = capture(
        Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--output",
            ])
            .arg(destination)
            .arg(url),
    )
    .with_context(|| format!("fetching {url}"))?;
    if !output.status.success() {
        bail!(
            "fetching {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Fetch, verify and unpack `published` in `staging`, answering the release
/// tree inside it.
fn unpack_verified(published: &Published, staging: &Path) -> Result<PathBuf> {
    let name = &published.archive_name;
    let archive = staging.join(name);
    let sums = staging.join("published-checksums.txt");
    fetch(&published.archive_url, &archive)?;
    fetch(&published.sums_url, &sums)?;
    let listed =
        std::fs::read_to_string(&sums).with_context(|| format!("reading {}", sums.display()))?;
    let expected = listed
        .lines()
        .find_map(|line| line.strip_suffix(&format!("  {name}")))
        .with_context(|| format!("{} lists no {name}", published.sums_url))?;
    let actual = sha256(&archive)?;
    if actual != expected {
        bail!(
            "{name} has sha256 {actual}; {} lists {expected}",
            published.sums_url
        );
    }
    let unpacked = staging.join("unpacked");
    unpack(&archive, &unpacked).with_context(|| format!("unpacking {name}"))?;
    Ok(match &published.tree {
        Some(tree) => unpacked.join(tree),
        None => unpacked,
    })
}

/// Install `published` as `root/<version>`, replacing an earlier copy of that
/// version, through a staging directory under `root` that is removed whatever
/// happened. Answers the installed directory.
pub fn install(published: &Published, root: &Path, version: &str) -> Result<PathBuf> {
    let staging = root.join(format!(".install-{}", std::process::id()));
    std::fs::create_dir_all(&staging).with_context(|| format!("creating {}", staging.display()))?;
    let destination = root.join(version);
    let placed = (|| -> Result<()> {
        let tree = unpack_verified(published, &staging)?;
        remove_tree(&destination)?;
        std::fs::rename(&tree, &destination)
            .with_context(|| format!("placing {}", destination.display()))
    })();
    std::fs::remove_dir_all(&staging).with_context(|| format!("removing {}", staging.display()))?;
    placed?;
    Ok(destination)
}
