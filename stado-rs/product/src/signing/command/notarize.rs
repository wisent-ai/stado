//! `stado product signing notarize --app PATH`: a signed macOS app bundle
//! submitted to Apple's notary service with the team's API key, the ticket
//! stapled into the bundle, and Gatekeeper's verdict checked. Eleven desktop
//! products carried the same `ditto`/`notarytool`/`stapler`/`spctl` block in
//! their release scripts; each now names this step.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};

use crate::release_steps::{output_dir, required};

fn run(program: &str, arguments: &[&str], path: &Path) -> Result<std::process::Output> {
    let output = Command::new(program)
        .args(arguments)
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {} {} failed with {}: {}{}",
            arguments.join(" "),
            path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(output)
}

/// The API key as bytes: the worker hands it base64-encoded, a PEM is taken as is.
fn api_key() -> Result<Vec<u8>> {
    let encoded = required("AC_API_KEY_P8")?;
    if encoded.contains("BEGIN PRIVATE KEY") {
        return Ok(encoded.into_bytes());
    }
    base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .context("AC_API_KEY_P8 is neither a PEM key nor base64")
}

/// Submit, staple, validate and assess `app`; the notary's JSON answer is
/// written to `evidence`. The zip and the key live in a directory of this
/// step's own under WISENT_OUTPUT_DIR and are removed however it ends.
pub fn notarize(app: &Path, evidence: Option<&Path>) -> Result<Vec<Value>> {
    if !app.is_dir() || app.extension().and_then(|e| e.to_str()) != Some("app") {
        bail!("--app must name a built .app bundle, not {}", app.display());
    }
    let key_id = required("AC_API_KEY_ID")?;
    let issuer = required("AC_API_ISSUER_ID")?;
    let key = api_key()?;
    let output = output_dir()?;
    let evidence: PathBuf = evidence.map_or_else(|| output.join("notary.json"), Path::to_path_buf);
    let work = output.join(format!("notarize-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work)?;
    let submitted = (|| -> Result<Value> {
        let key_path = work.join(format!("AuthKey_{key_id}.p8"));
        fs::write(&key_path, &key)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
        }
        let zip = work.join("notarize.zip");
        let zipped = Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(app)
            .arg(&zip)
            .status()
            .context("cannot run ditto")?;
        if !zipped.success() {
            bail!(
                "ditto could not zip {} for the notary: {zipped}",
                app.display()
            );
        }
        let key_arg = key_path.to_string_lossy().into_owned();
        let answer = run(
            "xcrun",
            &[
                "notarytool",
                "submit",
                "--key",
                &key_arg,
                "--key-id",
                &key_id,
                "--issuer",
                &issuer,
                "--wait",
                "--output-format",
                "json",
            ],
            &zip,
        )?;
        let text = String::from_utf8_lossy(&answer.stdout).trim().to_owned();
        let answer: Value = serde_json::from_str(&text)
            .with_context(|| format!("notarytool answered no JSON: {text}"))?;
        Ok(answer)
    })();
    let _ = fs::remove_dir_all(&work);
    let answer = submitted?;
    fs::write(&evidence, format!("{answer}\n"))?;
    if answer["status"] != "Accepted" {
        bail!(
            "Apple's notary did not accept {}: status {}, submission {} (answer in {})",
            app.display(),
            answer["status"],
            answer["id"],
            evidence.display()
        );
    }
    run("xcrun", &["stapler", "staple"], app)?;
    run("xcrun", &["stapler", "validate"], app)?;
    run(
        "spctl",
        &["--assess", "--type", "execute", "--verbose=2"],
        app,
    )?;
    Ok(vec![json!({
        "path": app,
        "state": "stable",
        "notarized": true,
        "submission": answer["id"],
        "evidence": evidence,
    })])
}
