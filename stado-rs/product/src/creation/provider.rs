use super::protocol::MAX_BYTES;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, io::Read, path::PathBuf};

pub fn ensure(repository: &str, request_id: &str, description: &str) -> Result<Value> {
    let origin = std::env::var("STADO_PRODUCT_INTEGRATION_URL")
        .context("STADO_PRODUCT_INTEGRATION_URL must name an HTTPS integration origin")?;
    let origin = origin.trim_end_matches('/');
    let parsed = url::Url::parse(origin)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        bail!("STADO_PRODUCT_INTEGRATION_URL must name an HTTPS integration origin");
    }
    let path = PathBuf::from(
        std::env::var_os("STADO_PRODUCT_INTEGRATION_TOKEN_FILE").context(
            "STADO_PRODUCT_INTEGRATION_TOKEN_FILE must name the owner-only caller grant",
        )?,
    );
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!(
            "integration caller grant must be an owner-only regular file: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            bail!("integration caller grant must be owned by this user and inaccessible to other users");
        }
    }
    let token = fs::read_to_string(path)?;
    let token = token.trim();
    if token.is_empty() || token.contains('\n') || token.contains('\r') {
        bail!("integration caller grant is empty or malformed");
    }
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client.post(format!("{origin}/api/integration/singularity/github_ensure_repo"))
        .bearer_auth(token).json(&json!({"repository": repository, "request_id": request_id, "description": description, "private": true}))
        .send().with_context(|| format!("provision {repository}: integration connection failed"))?;
    let status = response.status();
    let mut bytes = Vec::new();
    response
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        bail!("repository provisioning response exceeds its protocol bound");
    }
    if !status.is_success() {
        bail!(
            "provision {repository}: integration HTTP {}: {}",
            status.as_u16(),
            String::from_utf8_lossy(&bytes).replace(token, "[redacted]")
        );
    }
    let envelope: Value = serde_json::from_slice(&bytes)?;
    if envelope["ok"] != true || !envelope["result"].is_object() {
        bail!(
            "provision {repository}: integration refused: {}",
            envelope.to_string().replace(token, "[redacted]")
        );
    }
    let result = &envelope["result"];
    if result["full_name"] != repository
        || result["request_id"] != request_id
        || result["private"] != true
    {
        bail!("repository provisioning returned another request, identity or visibility");
    }
    Ok(result.clone())
}
