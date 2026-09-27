//! `stado product deliver sparkle`: a desktop release's update archive, its
//! Sparkle signature and the appcast uploaded where the installed apps look
//! for updates. The same Python file was copied into oko-, echo-, skarbiec-,
//! brama-, lem-, byk-, most-, probierz- and tama-desktop.

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};

use super::{output_dir, required, RECORD_SCHEMA};

/// Where the uploaded files are served from, as the receipt names them.
const PUBLIC_UPDATES: &str = "https://updates.wisent.ai";

/// The bytes of the one regular file named `basename` in the gzipped release.
pub(super) fn member(archive: &PathBuf, basename: &str) -> Result<Vec<u8>> {
    let mut bundle = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(archive)?));
    for entry in bundle.entries()? {
        let mut entry = entry?;
        let named = entry.path()?.file_name().and_then(|name| name.to_str()) == Some(basename);
        if named && entry.header().entry_type().is_file() {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
    }
    bail!(
        "the release archive {} holds no {basename}",
        archive.display()
    )
}

pub fn deliver() -> Result<i32> {
    let product = required("WISENT_PRODUCT")?;
    let version = required("WISENT_VERSION")?;
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let expected = required("WISENT_RELEASE_SHA256")?;
    if crate::common::sha256(&archive)? != expected {
        bail!(
            "the release archive {} is not the published {expected}; nothing was uploaded",
            archive.display()
        );
    }
    let contract_path =
        PathBuf::from(required("WISENT_SOURCE_DIR")?).join(".wisent-desktop-release.json");
    let contract: Value = serde_json::from_slice(
        &fs::read(&contract_path)
            .with_context(|| format!("reading {}", contract_path.display()))?,
    )?;
    let display = contract["product"]
        .as_str()
        .with_context(|| format!("{} names no product", contract_path.display()))?;
    let update = member(&archive, &format!("{display}.zip"))?;
    let signature = member(&archive, &format!("{display}.zip.sparkle-signature"))?;
    let appcast = member(&archive, "appcast.xml")?;
    let base = required("WISENT_SPARKLE_UPLOAD_BASE_URL")?
        .trim_end_matches('/')
        .to_owned();
    let token = required("WISENT_SPARKLE_TOKEN")?;
    let archive_name = format!("{display}-{version}.zip");
    let http = Client::builder().timeout(None).build()?;
    for (bytes, name, content_type) in [
        (&update, archive_name.clone(), "application/zip"),
        (
            &signature,
            format!("{archive_name}.sparkle-signature"),
            "text/plain",
        ),
        (&appcast, "appcast.xml".to_owned(), "application/xml"),
    ] {
        let response = http
            .put(format!("{base}/{product}/{name}"))
            .bearer_auth(&token)
            .header("Content-Type", content_type)
            .body(bytes.clone())
            .send()
            .with_context(|| format!("uploading {name}"))?;
        if !response.status().is_success() {
            let status = response.status();
            bail!(
                "Sparkle upload of {name} returned HTTP {status}: {}",
                response.text().unwrap_or_default()
            );
        }
    }
    let receipt = json!({
        "schema_version": RECORD_SCHEMA, "channel": "sparkle-appcast", "product": product,
        "version": version, "platform": required("WISENT_PLATFORM")?,
        "release_uri": required("WISENT_RELEASE_URI")?, "release_sha256": expected,
        "release_manifest_uri": required("WISENT_RELEASE_MANIFEST_URI")?,
        "release_manifest_sha256": required("WISENT_RELEASE_MANIFEST_SHA256")?,
        "archive_sha256": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&update)),
        "archive_url": format!("{PUBLIC_UPDATES}/{product}/{archive_name}"),
        "appcast_url": format!("{PUBLIC_UPDATES}/{product}/appcast.xml"),
    });
    fs::write(
        output_dir()?.join("sparkle-appcast-receipt.json"),
        format!("{receipt}\n"),
    )?;
    println!("uploaded {archive_name}, its signature and appcast.xml for {product}");
    Ok(0)
}
