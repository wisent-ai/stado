//! The components the compiled catalog declares under `public_status`, and
//! one look at each: the declared request, sent once, and what came back.
//!
//! A component is up when it answers the declared status and, when declared,
//! carries the declared text. Anything else is down with the answer itself as
//! the reason: the status and the body the component sent, or the transport's
//! own words when nothing came back, or why the request could not be made
//! (an address or a vault field that could not be read).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::cli::CmdError;

/// One declared check, in the shape the catalog validation admits.
#[derive(Deserialize)]
pub(super) struct Check {
    pub(super) component: String,
    pub(super) title: String,
    method: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    url_secret: Option<String>,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
    /// The status a working component answers, by its HTTP reason phrase
    /// (`OK`, `Unauthorized`), as `StatusCode::canonical_reason` names it.
    expect_status: String,
    #[serde(default)]
    expect_body: Option<String>,
}

/// A check and the product that declares it.
pub(super) struct Declared {
    pub(super) product: String,
    pub(super) check: Check,
}

/// What one look found: up or down, and the answer that says so.
pub(super) struct Looked {
    pub(super) up: bool,
    pub(super) detail: String,
}

/// Every check the compiled catalog declares, product by product. A catalog
/// that declares none is refused: publishing an empty page would read as
/// "nothing is wrong".
pub(super) fn declared() -> Result<Vec<Declared>, CmdError> {
    let catalog = stado_product::catalog::embedded().map_err(|error| {
        CmdError::click(format!(
            "the compiled product catalog is invalid: {error:#}"
        ))
    })?;
    let mut declared = Vec::new();
    for product in catalog["products"].as_array().into_iter().flatten() {
        let Some(checks) = product.get("public_status") else {
            continue;
        };
        let Some(id) = product["id"].as_str() else {
            return Err(CmdError::click(format!(
                "a catalogued product declaring public_status has no id: {product}"
            )));
        };
        let checks: Vec<Check> = serde_json::from_value(checks.clone()).map_err(|error| {
            CmdError::click(format!(
                "{id}.public_status in the compiled catalog is not a list of checks: {error}"
            ))
        })?;
        declared.extend(checks.into_iter().map(|check| Declared {
            product: id.to_owned(),
            check,
        }));
    }
    if declared.is_empty() {
        return Err(CmdError::click(
            "no catalogued product declares public_status, so there is no component to check \
             (catalog/products.yml declares them per product)",
        ));
    }
    Ok(declared)
}

/// The value of vault field `reference` (`role#field`), never printed.
async fn secret(reference: &str) -> Result<String, String> {
    let Some((role, field)) = reference.split_once('#') else {
        return Err(format!("{reference:?} is not role#field"));
    };
    let vault = crate::skarbiec::Client::stado()
        .map_err(|error| format!("Skarbiec cannot be asked for {reference}: {error}"))?;
    let value = vault
        .read_field(role, field)
        .await
        .map_err(|error| format!("Skarbiec did not give {reference}: {error}"))?;
    match value.as_str().filter(|value| !value.is_empty()) {
        Some(value) => Ok(value.to_owned()),
        None => Err(format!("Skarbiec holds no non-empty string at {reference}")),
    }
}

/// The address this host reaches `service` at: its forward marker
/// (`~/.stado/forwards/<service>.local`), which `stado service directory
/// publish` writes from the service directory.
fn forwarded(service: &str) -> Result<String, String> {
    let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) else {
        return Err(format!(
            "HOME is not set, so the forward marker of {service} cannot be found"
        ));
    };
    let marker = PathBuf::from(home)
        .join(".stado")
        .join("forwards")
        .join(format!("{service}.local"));
    let address = std::fs::read_to_string(&marker).map_err(|error| {
        format!(
            "the forward marker {} of {service} cannot be read ({error}); `stado service \
             directory publish` writes it",
            marker.display()
        )
    })?;
    match address.trim() {
        "" => Err(format!("the forward marker {} is empty", marker.display())),
        address => Ok(address.to_owned()),
    }
}

/// `base` and `path` joined with one slash between them.
fn joined(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

/// The URL the check asks.
async fn address(check: &Check) -> Result<String, String> {
    let path = || match &check.path {
        Some(path) => Ok(path.as_str()),
        None => Err(format!(
            "the check {} names no path for its address",
            check.component
        )),
    };
    match (&check.url, &check.url_secret, &check.service) {
        (Some(url), None, None) => Ok(url.clone()),
        (None, Some(reference), None) => Ok(joined(&secret(reference).await?, path()?)),
        (None, None, Some(service)) => Ok(joined(&forwarded(service)?, path()?)),
        _ => Err(format!(
            "the check {} declares more than one address, or none",
            check.component
        )),
    }
}

/// The body as one line: a component's error page keeps every word.
fn flattened(body: &str) -> String {
    body.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn ask(check: &Check) -> Result<String, String> {
    let url = address(check).await?;
    let method = reqwest::Method::from_bytes(check.method.as_bytes())
        .map_err(|error| format!("{} is not an HTTP method: {error}", check.method))?;
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("no HTTP client could be built: {error}"))?;
    let mut request = client.request(method, &url);
    for (name, reference) in &check.headers {
        request = request.header(name.as_str(), secret(reference).await?);
    }
    if let Some(body) = &check.body {
        request = request.json(body);
    }
    let response = crate::wait::request(request)
        .await
        .map_err(|error| format!("{} {url} got no answer: {error}", check.method))?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        format!(
            "{} {url} answered {status} and its body could not be read: {error}",
            check.method
        )
    })?;
    if status.canonical_reason() != Some(check.expect_status.as_str()) {
        return Err(format!(
            "{} {url} answered {status} where a working component answers {}: {}",
            check.method,
            check.expect_status,
            flattened(&body)
        ));
    }
    if let Some(expected) = &check.expect_body {
        if !body.contains(expected.as_str()) {
            return Err(format!(
                "{} {url} answered {status} without {expected:?} in its body: {}",
                check.method,
                flattened(&body)
            ));
        }
    }
    Ok(format!("{} {url} answered {status}", check.method))
}

/// One look at `check`.
pub(super) async fn look(check: &Check) -> Looked {
    match ask(check).await {
        Ok(detail) => Looked { up: true, detail },
        Err(detail) => Looked { up: false, detail },
    }
}
