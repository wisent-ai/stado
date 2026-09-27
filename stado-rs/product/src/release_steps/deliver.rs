//! `stado product deliver render` and `stado product deliver vercel-files`:
//! the two hosting deliveries growth-tactics' copied script performed.
//!
//! Neither waits on a clock. A Vercel deployment is followed through its own
//! event stream, which the API closes when the build ends, and its state is
//! then read once. Render offers no stream to follow, so its delivery starts
//! the deploy and records the status Render answered with; whether that deploy
//! went live is Render's to report (`render-evidence.json` names the deploy).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

use super::python::{find, safe_unpack};
use super::{output_dir, required};

const RENDER_API: &str = "https://api.render.com/v1";
const VERCEL_API: &str = "https://api.vercel.com";

pub fn run(action: &str, arguments: &clap::ArgMatches) -> Result<i32> {
    let text = |name: &str| -> Result<String> {
        arguments
            .get_one::<String>(name)
            .cloned()
            .with_context(|| format!("--{name} is required"))
    };
    match action {
        "render" => render(&text("service-name")?),
        "sparkle" => super::sparkle::deliver(),
        "vercel-files" => vercel_files(
            &text("bundle")?,
            &text("team-id")?,
            &text("project-id")?,
            &text("project-name")?,
        ),
        other => bail!("unknown delivery {other}"),
    }
}

/// A client that lets a provider's response run to completion: the default
/// blocking client would cut a followed event stream off mid-build.
fn client() -> Result<Client> {
    Ok(Client::builder().timeout(None).build()?)
}

/// Send one provider request and answer its JSON, refusing a non-success
/// status with the provider's own body.
fn answer(provider: &str, request: reqwest::blocking::RequestBuilder) -> Result<Value> {
    let response = request
        .header("Accept", "application/json")
        .send()
        .with_context(|| format!("{provider} API request failed"))?;
    let status = response.status();
    let body = response.text()?;
    if !status.is_success() {
        bail!("{provider} API {status}: {body}");
    }
    if body.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&body).with_context(|| format!("{provider} answered non-JSON: {body}"))
}

fn release_evidence() -> Result<serde_json::Map<String, Value>> {
    let mut evidence = serde_json::Map::new();
    evidence.insert("release_uri".into(), json!(required("WISENT_RELEASE_URI")?));
    evidence.insert(
        "release_sha256".into(),
        json!(required("WISENT_RELEASE_SHA256")?),
    );
    Ok(evidence)
}

fn write_evidence(name: &str, evidence: serde_json::Map<String, Value>) -> Result<()> {
    let path = output_dir()?.join(name);
    fs::write(&path, format!("{}\n", Value::Object(evidence)))
        .with_context(|| format!("writing {}", path.display()))
}

fn render(service_name: &str) -> Result<i32> {
    let token = required("RENDER_API_KEY")?;
    let http = client()?;
    let services = answer(
        "Render",
        http.get(format!("{RENDER_API}/services"))
            .query(&[("name", service_name), ("limit", "100")])
            .bearer_auth(&token),
    )?;
    let exact: Vec<&Value> = services
        .as_array()
        .context("Render's service listing was not an array")?
        .iter()
        .map(|entry| entry.get("service").unwrap_or(entry))
        .filter(|service| service["name"] == service_name && service["id"].is_string())
        .collect();
    let [service] = exact.as_slice() else {
        bail!(
            "Render holds {} services named {service_name}; exactly one is required",
            exact.len()
        );
    };
    let service_id = service["id"].as_str().unwrap().to_owned();
    let deploy = answer(
        "Render",
        http.post(format!("{RENDER_API}/services/{service_id}/deploys"))
            .bearer_auth(&token)
            .json(&json!({"clearCache": "do_not_clear"})),
    )?;
    let deploy_id = deploy["id"]
        .as_str()
        .context("Render did not return a deploy id")?
        .to_owned();
    let status = deploy["status"].as_str().unwrap_or("unknown").to_owned();
    if matches!(
        status.as_str(),
        "build_failed" | "update_failed" | "canceled" | "deactivated"
    ) {
        bail!("Render deploy {deploy_id} of {service_name} was refused: {status}");
    }
    let mut evidence = release_evidence()?;
    evidence.insert("provider".into(), json!("render"));
    evidence.insert("service_id".into(), json!(service_id));
    evidence.insert("deploy_id".into(), json!(deploy_id));
    evidence.insert("status_at_start".into(), json!(status));
    write_evidence("render-evidence.json", evidence)?;
    println!("Render deploy {deploy_id} of {service_name} started ({status})");
    Ok(0)
}

fn files_under(directory: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            files_under(&path, found)?;
        } else {
            found.push(path);
        }
    }
    Ok(())
}

fn vercel_files(bundle_name: &str, team: &str, project: &str, project_name: &str) -> Result<i32> {
    let token = required("VERCEL_TOKEN")?;
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let release_sha256 = required("WISENT_RELEASE_SHA256")?;
    let actual = crate::common::sha256(&archive)?;
    if actual != release_sha256 {
        bail!(
            "the release archive {} is {actual}, not the published {release_sha256}; nothing was deployed",
            archive.display()
        );
    }
    let work = output_dir()?.join(format!("vercel-files-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work)?;
    let result = (|| -> Result<(String, Value)> {
        let bundle = if archive.file_name().and_then(|n| n.to_str()) == Some(bundle_name) {
            archive.clone()
        } else {
            let release = work.join("release");
            fs::create_dir_all(&release)?;
            safe_unpack(&archive, &release)?;
            match find(&release, bundle_name)?.as_slice() {
                [bundle] => bundle.clone(),
                found => bail!(
                    "the release holds {} {bundle_name} (one is required)",
                    found.len()
                ),
            }
        };
        let unpacked = work.join("bundle");
        fs::create_dir_all(&unpacked)?;
        safe_unpack(&bundle, &unpacked)?;
        let source = unpacked.join("source");
        let mut paths = Vec::new();
        files_under(&source, &mut paths)?;
        paths.sort();
        let http = client()?;
        let mut files = Vec::new();
        for path in &paths {
            let bytes = fs::read(path)?;
            let digest = hex::encode(Sha1::digest(&bytes));
            let size = bytes.len();
            answer(
                "Vercel",
                http.post(format!("{VERCEL_API}/v2/files"))
                    .query(&[("teamId", team)])
                    .bearer_auth(&token)
                    .header("Content-Type", "application/octet-stream")
                    .header("x-vercel-digest", &digest)
                    .body(bytes),
            )?;
            let relative = path
                .strip_prefix(&source)?
                .to_string_lossy()
                .replace('\\', "/");
            files.push(json!({"file": relative, "sha": digest, "size": size}));
        }
        let created = answer(
            "Vercel",
            http.post(format!("{VERCEL_API}/v13/deployments"))
                .query(&[("teamId", team)])
                .bearer_auth(&token)
                .json(&json!({
                    "name": project_name,
                    "project": project,
                    "target": "production",
                    "files": files,
                    "meta": {
                        "wisentReleaseUri": required("WISENT_RELEASE_URI")?,
                        "wisentReleaseSha256": release_sha256,
                    },
                })),
        )?;
        let id = created["id"]
            .as_str()
            .context("Vercel did not return a deployment id")?
            .to_owned();
        // The followed event stream ends when the build does.
        let mut stream = http
            .get(format!("{VERCEL_API}/v3/deployments/{id}/events"))
            .query(&[("teamId", team), ("follow", "1")])
            .bearer_auth(&token)
            .send()
            .context("following the Vercel deployment's events failed")?;
        if !stream.status().is_success() {
            bail!(
                "Vercel event stream {}: {}",
                stream.status(),
                stream.text()?
            );
        }
        std::io::copy(&mut stream.by_ref(), &mut std::io::sink())?;
        let deployment = answer(
            "Vercel",
            http.get(format!("{VERCEL_API}/v13/deployments/{id}"))
                .query(&[("teamId", team)])
                .bearer_auth(&token),
        )?;
        let state = deployment["readyState"]
            .as_str()
            .or(deployment["state"].as_str())
            .unwrap_or("unknown");
        if state != "READY" {
            bail!("Vercel deployment {id} ended in {state}");
        }
        Ok((id, deployment["url"].clone()))
    })();
    let _ = fs::remove_dir_all(&work);
    let (id, url) = result?;
    let mut evidence = release_evidence()?;
    evidence.insert("provider".into(), json!("vercel"));
    evidence.insert("deployment_id".into(), json!(id));
    evidence.insert("url".into(), url);
    evidence.insert("ready_state".into(), json!("READY"));
    write_evidence("vercel-evidence.json", evidence)?;
    println!("Vercel deployment {id} is ready");
    Ok(0)
}
