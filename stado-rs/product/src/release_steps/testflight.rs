//! `stado product deliver testflight --ipa NAME`: an iOS release's `.ipa`
//! uploaded to App Store Connect with the team's API key. The same Python
//! file, differing only in the `.ipa` name, was copied into jeden-, oko-,
//! byk- and wisent-ios.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};

use super::{output_dir, required, RECORD_SCHEMA};

/// The bytes of the one regular file named `basename` in the gzipped release.
fn member(archive: &PathBuf, basename: &str) -> Result<Vec<u8>> {
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

pub fn deliver(ipa_name: &str) -> Result<i32> {
    if !ipa_name.ends_with(".ipa") || ipa_name.contains('/') {
        bail!("--ipa is the file name of the release's .ipa, not {ipa_name:?}");
    }
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let expected = required("WISENT_RELEASE_SHA256")?;
    if crate::common::sha256(&archive)? != expected {
        bail!(
            "the release archive {} is not the published {expected}; nothing was uploaded",
            archive.display()
        );
    }
    let output = output_dir()?;
    let ipa = output.join(ipa_name);
    fs::write(&ipa, member(&archive, ipa_name)?)?;
    let key_id = required("AC_API_KEY_ID")?;
    let issuer = required("AC_API_ISSUER_ID")?;
    let encoded = required("AC_API_KEY_P8")?;
    let key = if encoded.contains("BEGIN PRIVATE KEY") {
        encoded.into_bytes()
    } else {
        base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .context("AC_API_KEY_P8 is neither a PEM key nor base64")?
    };
    // altool reads the key from ~/.appstoreconnect/private_keys; a home of the
    // delivery's own keeps it out of the builder's real home and is removed
    // whichever way the upload ends.
    let home = output.join(format!("testflight-home-{}", uuid::Uuid::new_v4()));
    let keys = home.join(".appstoreconnect/private_keys");
    fs::create_dir_all(&keys)?;
    let key_path = keys.join(format!("AuthKey_{key_id}.p8"));
    let uploaded = (|| -> Result<std::process::Output> {
        fs::write(&key_path, &key)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
        }
        Command::new("xcrun")
            .args(["altool", "--upload-app", "--type", "ios", "--file"])
            .arg(&ipa)
            .args([
                "--apiKey",
                &key_id,
                "--apiIssuer",
                &issuer,
                "--output-format",
                "json",
            ])
            .env("HOME", &home)
            .stdin(Stdio::null())
            .output()
            .context("cannot run xcrun altool")
    })();
    let _ = fs::remove_dir_all(&home);
    let uploaded = uploaded?;
    if !uploaded.status.success() {
        bail!(
            "xcrun altool --upload-app failed with {}: {}{}",
            uploaded.status,
            String::from_utf8_lossy(&uploaded.stderr).trim(),
            String::from_utf8_lossy(&uploaded.stdout).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&uploaded.stdout).trim().to_owned();
    let provider: Value =
        serde_json::from_str(&stdout).unwrap_or_else(|_| json!({"output": stdout}));
    let receipt = json!({
        "schema_version": RECORD_SCHEMA, "channel": "testflight",
        "product": required("WISENT_PRODUCT")?, "version": required("WISENT_VERSION")?,
        "platform": required("WISENT_PLATFORM")?, "release_uri": required("WISENT_RELEASE_URI")?,
        "release_sha256": expected,
        "release_manifest_uri": required("WISENT_RELEASE_MANIFEST_URI")?,
        "release_manifest_sha256": required("WISENT_RELEASE_MANIFEST_SHA256")?,
        "provider_receipt": provider,
    });
    fs::write(
        output.join("testflight-receipt.json"),
        format!("{receipt}\n"),
    )?;
    println!("uploaded {ipa_name} to App Store Connect");
    Ok(0)
}
