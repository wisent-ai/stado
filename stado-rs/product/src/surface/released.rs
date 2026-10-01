//! Which revision of a Swift package consumers resolve.
//!
//! SwiftPM resolves `from:` to a git tag, so the released artifact is a tag at
//! the remote; a published GitHub release outranks a bare tag. Tags are read
//! from the remote, never from the local listing: a runner checks out one
//! commit and no tags, and a listing read there calls the bottom tier best at
//! the moment a tag appears. A refusal from either read stays a refusal;
//! silence is never read as absence.

use crate::common::checked;
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const REMOTE: &str = "origin";

/// A version as a tag names it: `v1.2.3` or `1.2.3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn parse(tag: &str) -> Option<Version> {
        let core = tag.strip_prefix('v').unwrap_or(tag);
        let mut parts = core.split('.').map(str::parse::<u64>);
        let version = Version {
            major: parts.next()?.ok()?,
            minor: parts.next().transpose().ok()?.unwrap_or(0),
            patch: parts.next().transpose().ok()?.unwrap_or(0),
        };
        parts.next().is_none().then_some(version)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// How the released surface was reached, written into the report.
#[derive(Clone, Debug)]
pub enum Artifact {
    Release { tag: String },
    Tag { tag: String },
    Nothing,
}

impl Artifact {
    pub fn tag(&self) -> Option<&str> {
        match self {
            Artifact::Release { tag } | Artifact::Tag { tag } => Some(tag),
            Artifact::Nothing => None,
        }
    }
}

fn git(root: &Path, arguments: &[&str]) -> Result<String> {
    let output = checked(Command::new("git").arg("-C").arg(root).args(arguments))?;
    Ok(String::from_utf8(output.stdout)?)
}

/// `owner/name` of the remote, from `GITHUB_REPOSITORY` on a runner or the
/// remote URL elsewhere.
pub fn slug(root: &Path) -> Result<String> {
    if let Ok(supplied) = std::env::var("GITHUB_REPOSITORY") {
        let supplied = supplied.trim().to_owned();
        let (owner, name) = supplied
            .split_once('/')
            .with_context(|| format!("GITHUB_REPOSITORY={supplied:?} is not owner/name"))?;
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            bail!("GITHUB_REPOSITORY={supplied:?} is not owner/name");
        }
        return Ok(supplied);
    }
    let url = git(root, &["remote", "get-url", REMOTE])?;
    let url = url.trim().trim_end_matches(".git");
    let mut segments = url.rsplit(['/', ':']);
    let name = segments.next().filter(|name| !name.is_empty());
    let owner = segments.next().filter(|owner| !owner.is_empty());
    match (owner, name) {
        (Some(owner), Some(name)) => Ok(format!("{owner}/{name}")),
        _ => bail!("cannot read owner/name out of the {REMOTE} URL {url:?}"),
    }
}

/// Tag names as the remote serves them.
pub fn remote_tags(root: &Path) -> Result<Vec<String>> {
    let listing = git(root, &["ls-remote", "--tags", REMOTE])?;
    let mut found: Vec<String> = Vec::new();
    for line in listing.lines() {
        let Some(reference) = line.split_whitespace().nth(1) else {
            continue;
        };
        let Some(name) = reference.strip_prefix("refs/tags/") else {
            continue;
        };
        let name = name.strip_suffix("^{}").unwrap_or(name);
        if !found.iter().any(|known| known == name) {
            found.push(name.to_owned());
        }
    }
    Ok(found)
}

fn api(slug_path: &str) -> Result<(StatusCode, Value)> {
    let mut request = Client::new()
        .get(format!("https://api.github.com/repos/{slug_path}"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "stado-product-surface");
    if let Some(token) = std::env::var("GH_TOKEN")
        .ok()
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .filter(|token| !token.is_empty())
    {
        request = request.bearer_auth(token);
    }
    let response = request
        .send()
        .with_context(|| format!("no answer from the GitHub API for {slug_path}; absence is not proven by a request that did not complete"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .with_context(|| format!("the GitHub API for {slug_path} did not answer with JSON"))?;
    Ok((status, body))
}

/// Non-draft release tags, after proving the API names this repository back
/// and lists the tags the remote serves.
pub fn published_release_tags(root: &Path, slug: &str, over_git: &[String]) -> Result<Vec<String>> {
    let (status, repository) = api(slug)?;
    if status != StatusCode::OK {
        bail!("the GitHub API answered {status} for {slug}, so this read cannot see the repository it asks about and every absence it reports is unproven");
    }
    let named = repository["full_name"].as_str().unwrap_or("");
    if !named.eq_ignore_ascii_case(slug) {
        bail!("the GitHub API did not name {slug} back (it answered {named:?}), so this read is not looking at the repository it believes it is");
    }
    if !over_git.is_empty() {
        let (status, tags) = api(&format!("{slug}/tags?per_page=100"))?;
        if status != StatusCode::OK || tags.as_array().is_none_or(Vec::is_empty) {
            bail!(
                "{REMOTE} lists {} tags for {slug} but the API's tag list answered {status} with none, so it is silent about a fact that holds and no absence it reports can be believed",
                over_git.len()
            );
        }
    }
    let (status, releases) = api(&format!("{slug}/releases?per_page=100"))?;
    if status == StatusCode::NOT_FOUND {
        bail!("the releases API says {slug} does not exist: a wrong subject, not an absence of releases");
    }
    if status != StatusCode::OK {
        bail!("the releases API answered {status} for {slug}, which states neither presence nor absence");
    }
    let entries = releases
        .as_array()
        .with_context(|| format!("the releases API for {slug} did not answer with a list"))?;
    Ok(entries
        .iter()
        .filter(|entry| !entry["draft"].as_bool().unwrap_or(false))
        .filter_map(|entry| entry["tag_name"].as_str().map(str::to_owned))
        .collect())
}

/// The newest tag that names a version.
pub fn newest(tags: &[String]) -> Option<(String, Version)> {
    tags.iter()
        .filter_map(|tag| Version::parse(tag).map(|version| (tag.clone(), version)))
        .max_by_key(|(_, version)| *version)
}

/// The released artifact: a published release, else a remote tag, else none.
pub fn artifact(root: &Path) -> Result<(Artifact, Option<Version>)> {
    let tags = remote_tags(root)?;
    let releases = published_release_tags(root, &slug(root)?, &tags)?;
    if let Some((tag, version)) = newest(&releases) {
        return Ok((Artifact::Release { tag }, Some(version)));
    }
    if let Some((tag, version)) = newest(&tags) {
        return Ok((Artifact::Tag { tag }, Some(version)));
    }
    Ok((Artifact::Nothing, None))
}

/// Unpack `tag`'s tree under `destination`, after proving the local tag
/// names the commit the remote serves.
pub fn checkout_tag(root: &Path, tag: &str, destination: &Path) -> Result<PathBuf> {
    let local = git(root, &["rev-parse", &format!("{tag}^{{commit}}")])?;
    let listing = git(root, &["ls-remote", REMOTE, &format!("refs/tags/{tag}")])?;
    let mut peeled = Vec::new();
    for object in listing
        .lines()
        .filter_map(|line| line.split_whitespace().next())
    {
        peeled.push(
            git(root, &["rev-parse", &format!("{object}^{{commit}}")])?
                .trim()
                .to_owned(),
        );
    }
    if peeled.is_empty() {
        bail!("the remote does not serve tag {tag}, so it is not an artifact anyone resolved");
    }
    if !peeled.contains(&local.trim().to_owned()) {
        bail!("tag {tag} is {} here but {peeled:?} at the remote; refusing to measure against a tag whose identity is not settled", local.trim());
    }
    if destination.exists() {
        std::fs::remove_dir_all(destination)?;
    }
    std::fs::create_dir_all(destination)?;
    let archive =
        checked(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(["archive", "--format=tar", tag]),
        )?;
    tar::Archive::new(archive.stdout.as_slice())
        .unpack(destination)
        .with_context(|| format!("unpacking the {tag} archive into {}", destination.display()))?;
    Ok(destination.to_path_buf())
}

/// The version this revision declares: the newest version tag pointing at
/// it, else `version.json`'s `version`, which the release pipeline reads
/// and tags.
pub fn declared(root: &Path) -> Result<Option<Version>> {
    let listing = git(root, &["tag", "--points-at", "HEAD"])?;
    let tags: Vec<String> = listing.lines().map(str::to_owned).collect();
    if let Some((_, version)) = newest(&tags) {
        return Ok(Some(version));
    }
    let file = root.join("version.json");
    if !file.is_file() {
        return Ok(None);
    }
    let document: Value = serde_json::from_str(&std::fs::read_to_string(&file)?)
        .with_context(|| format!("{} is not JSON", file.display()))?;
    let text = document["version"]
        .as_str()
        .with_context(|| format!("{} has no version string", file.display()))?;
    Version::parse(text)
        .map(Some)
        .with_context(|| format!("{} does not hold a version: {text:?}", file.display()))
}
