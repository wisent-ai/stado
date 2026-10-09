//! `stado product deliver github-mirror --repository OWNER/NAME --title T
//! [--signed-binary PATH]`: an optional mirror of a published release on the
//! product's GitHub repository: the tag `v<version>` at the release's source
//! revision, a GitHub release, and the release archive as its asset. The same
//! Python file was copied into oko and weles-client; oko's copy also refused a
//! binary that is not Developer ID signed, which `--signed-binary` keeps.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::StatusCode;
use serde_json::{json, Value};
use stado_wait as wait;

use super::super::{output_dir, required, RECORD_SCHEMA};

const API: &str = "https://api.github.com/repos";
const UPLOADS: &str = "https://uploads.github.com/repos";
/// A full git commit id.
const REVISION_LENGTH: usize = 40;

fn github(request: RequestBuilder, token: &str) -> RequestBuilder {
    request
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "stado-release-delivery")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

/// `Ok(None)` for a 404, the answer for a tag or release not there yet.
fn read(request: RequestBuilder, token: &str) -> Result<Option<Value>> {
    let response = wait::request_blocking(github(request, token))?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = response.status();
    let body = response.text()?;
    if !status.is_success() {
        bail!("GitHub answered {status}: {body}");
    }
    Ok(Some(serde_json::from_str(&body)?))
}

/// The archive member whose path is `suffix` or ends in `/suffix`, as bytes.
fn member_ending(archive: &Path, suffix: &str) -> Result<Vec<u8>> {
    let nested = format!("/{suffix}");
    let mut bundle = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(archive)?));
    for entry in bundle.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if entry.header().entry_type().is_file() && (path == suffix || path.ends_with(&nested)) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
    }
    bail!(
        "the release archive {} holds no {suffix}",
        archive.display()
    )
}

/// The signing authority of `binary` in the release, refused unless it is a
/// valid Developer ID signature: an ad-hoc binary must not be published.
fn developer_id(archive: &Path, binary: &str, work: &Path) -> Result<String> {
    let target = work.join(
        Path::new(binary)
            .file_name()
            .context("--signed-binary names no file")?,
    );
    fs::write(&target, member_ending(archive, binary)?)?;
    let verified = wait::output(
        Command::new("codesign")
            .args(["--verify", "--strict"])
            .arg(&target),
    )?;
    if !verified.status.success() {
        bail!(
            "{binary} is not validly signed, so it cannot be published: {}",
            String::from_utf8_lossy(&verified.stderr).trim()
        );
    }
    let shown = wait::output(
        Command::new("codesign")
            .args(["--display", "--verbose=2"])
            .arg(&target),
    )?;
    let authority = String::from_utf8_lossy(&shown.stderr)
        .lines()
        .find_map(|line| line.strip_prefix("Authority=").map(str::to_owned))
        .unwrap_or_default();
    if !authority.starts_with("Developer ID Application") {
        bail!("{binary} is signed by {authority:?}, not a Developer ID; it must not be published");
    }
    Ok(authority)
}

pub fn deliver(repository: &str, title: &str, signed_binary: Option<&str>) -> Result<i32> {
    if repository.split('/').count() != 2 {
        bail!("--repository is OWNER/NAME, not {repository:?}");
    }
    let token = required("WISENT_MIRROR_TOKEN")?;
    let version = required("WISENT_VERSION")?;
    let platform = required("WISENT_PLATFORM")?;
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let release_uri = required("WISENT_RELEASE_URI")?;
    let release_sha256 = required("WISENT_RELEASE_SHA256")?;
    if crate::common::sha256(&archive)? != release_sha256 {
        bail!(
            "the release archive {} is not the published {release_sha256}; nothing was mirrored",
            archive.display()
        );
    }
    let output = output_dir()?;
    let authority = match signed_binary {
        Some(binary) => {
            let work = output.join(format!("signature-check-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&work)?;
            let checked = developer_id(&archive, binary, &work);
            let _ = fs::remove_dir_all(&work);
            Some(checked?)
        }
        None => None,
    };
    let revision = String::from_utf8(member_ending(&archive, "SOURCE_REVISION")?)?
        .trim()
        .to_owned();
    if revision.len() != REVISION_LENGTH
        || !revision
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        bail!("the release archive carries an invalid source revision {revision:?}");
    }
    let http = Client::builder().timeout(None).build()?;
    let tag = format!("v{version}");
    match read(
        http.get(format!("{API}/{repository}/git/ref/tags/{tag}")),
        &token,
    )? {
        Some(existing) if existing["object"]["sha"] != revision => {
            bail!(
                "{tag} already exists at {} and does not identify the release's source {revision}",
                existing["object"]["sha"]
            );
        }
        Some(_) => {}
        None => {
            read(
                http.post(format!("{API}/{repository}/git/refs"))
                    .json(&json!({"ref": format!("refs/tags/{tag}"), "sha": revision})),
                &token,
            )?;
        }
    }
    let release = match read(http.get(format!("{API}/{repository}/releases/tags/{tag}")), &token)? {
        Some(release) => release,
        None => read(
            http.post(format!("{API}/{repository}/releases")).json(&json!({
                "tag_name": tag, "name": format!("{title} {version}"),
                "body": format!("Optional mirror of {release_uri}"), "draft": false, "prerelease": false,
            })),
            &token,
        )?
        .context("GitHub created no release")?,
    };
    let product = required("WISENT_PRODUCT")?;
    let asset = format!("{product}-{version}-{platform}.tar.gz");
    let present = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|existing| existing["name"] == asset);
    if !present {
        read(
            http.post(format!(
                "{UPLOADS}/{repository}/releases/{}/assets",
                release["id"]
            ))
            .query(&[("name", asset.as_str())])
            .header("Content-Type", "application/gzip")
            .body(fs::read(&archive)?),
            &token,
        )?;
    }
    let receipt = json!({
        "schema_version": RECORD_SCHEMA, "channel": "github-mirror", "product": product,
        "version": version, "platform": platform, "source_revision": revision,
        "release_uri": release_uri, "release_sha256": release_sha256,
        "release_manifest_uri": required("WISENT_RELEASE_MANIFEST_URI")?,
        "release_manifest_sha256": required("WISENT_RELEASE_MANIFEST_SHA256")?,
        "external_url": release["html_url"], "asset": asset, "signing_authority": authority,
    });
    fs::write(
        output.join("github-mirror-receipt.json"),
        format!("{receipt}\n"),
    )?;
    println!(
        "mirrored {asset} to {}",
        release["html_url"].as_str().unwrap_or(repository)
    );
    Ok(0)
}
