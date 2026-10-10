//! Edge secrets are names in the verified bundle, values in the delivery's
//! injected environment. No value is accepted from source or written to a receipt.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{json, Value};
use stado_wait as wait;

#[derive(Serialize)]
pub(super) struct Secret {
    name: String,
    value: String,
}

pub(super) fn load(source: &Path) -> Result<Vec<Secret>> {
    let declaration = source.join("supabase/function-secrets.json");
    let bytes = match fs::read(&declaration) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", declaration.display()))
        }
    };
    let names: Vec<String> = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "{} must be a JSON array of environment names",
            declaration.display()
        )
    })?;
    let mut seen = HashSet::with_capacity(names.len());
    for name in &names {
        let mut characters = name.chars();
        if !characters
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !characters.all(|c| c.is_ascii_alphanumeric() || c == '_')
            || name.starts_with("SUPABASE_")
        {
            bail!(
                "{} declares invalid or provider-reserved function secret {name:?}",
                declaration.display()
            );
        }
        if !seen.insert(name.as_str()) {
            bail!("{} repeats function secret {name}", declaration.display());
        }
    }
    drop(seen);
    names
        .into_iter()
        .map(|name| {
            let value = super::super::required(&name).with_context(|| {
                format!("resolving declared function secret {name} before schema delivery")
            })?;
            Ok(Secret { name, value })
        })
        .collect()
}

pub(super) fn publish(secrets: &[Secret], project: &str, token: &str) -> Result<Value> {
    if secrets.is_empty() {
        return Ok(json!({ "names": [], "state": "not_declared" }));
    }
    // Supabase Management API: POST /v1/projects/{ref}/secrets. The provider's
    // origin is the API contract, not a project or fleet placement default.
    let mut endpoint = reqwest::Url::parse("https://api.supabase.com/v1/projects/")?;
    endpoint
        .path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Supabase management URL cannot hold path segments"))?
        .pop_if_empty()
        .push(project)
        .push("secrets");
    let client = Client::builder()
        .timeout(None)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = wait::request_blocking(
        client
            .post(endpoint.clone())
            .bearer_auth(token)
            .json(secrets),
    )
    .with_context(|| format!("POST {endpoint}: publishing declared function secrets failed"))?;
    let status = response.status();
    let body = response
        .text()
        .with_context(|| format!("POST {endpoint}: reading HTTP {status} response"))?;
    if !status.is_success() {
        bail!("POST {endpoint}: HTTP {status}: {body}");
    }
    let names: Vec<&str> = secrets.iter().map(|secret| secret.name.as_str()).collect();
    println!(
        "Supabase project {project} accepted function secrets: {}",
        names.join(", ")
    );
    Ok(json!({ "names": names, "state": "accepted", "http_status": status.as_u16() }))
}
