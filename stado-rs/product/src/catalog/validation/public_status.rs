//! A product's `public_status`: the components a person relies on when they
//! use it, each with the one request that says whether it works for them.
//!
//! Every check names a component (a slug unique in the product) and its
//! title, the HTTP method, exactly one address — an absolute HTTPS `url`, a
//! base URL read from the vault by role (`url_secret`, `role#field`) or the
//! service directory entry this host reaches (`service`) — with the `path`
//! the two indirect addresses need, request headers whose values are vault
//! fields by role, an optional JSON `body`, the status the component answers
//! when it works (`expect_status`) and optionally a text its answer carries
//! (`expect_body`). Stado runs them (`stado availability check`) and
//! publishes what each answered; nothing else declares a component.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::HashSet;

use super::super::text;
use crate::common::slug;

/// `role#field`, both parts non-empty.
fn reference(value: &str, place: &str) -> Result<()> {
    match value.split_once('#') {
        Some((role, field)) if !role.is_empty() && !field.is_empty() => Ok(()),
        _ => bail!("{place}: {value:?} is not a vault reference role#field"),
    }
}

pub(super) fn validate(id: &str, checks: &Value) -> Result<()> {
    let checks = checks
        .as_array()
        .with_context(|| format!("{id}.public_status must be a list"))?;
    if checks.is_empty() {
        bail!("{id}.public_status: a declared list names at least one component");
    }
    let mut components = HashSet::new();
    for check in checks {
        let component = text(check, "component")?;
        slug(component)?;
        if !components.insert(component) {
            bail!("{id}.public_status: duplicate component {component}");
        }
        let place = format!("{id}.public_status.{component}");
        text(check, "title")?;
        let method = text(check, "method")?;
        if reqwest::Method::from_bytes(method.as_bytes()).is_err()
            || method.chars().any(|letter| !letter.is_ascii_uppercase())
        {
            bail!("{place}: method {method:?} is not an HTTP method in capitals");
        }
        let declared: Vec<&str> = ["url", "url_secret", "service"]
            .into_iter()
            .filter(|field| check.get(*field).is_some())
            .collect();
        let [address] = declared.as_slice() else {
            bail!("{place}: declare exactly one of url, url_secret and service, not {declared:?}");
        };
        match *address {
            "url" => {
                let url = url::Url::parse(text(check, "url")?)
                    .with_context(|| format!("{place}: url is not a URL"))?;
                if url.scheme() != "https" || url.host_str().is_none() {
                    bail!("{place}: url must be an absolute HTTPS URL");
                }
                if check.get("path").is_some() {
                    bail!(
                        "{place}: a url carries its own path; path belongs to url_secret and \
                         service"
                    );
                }
            }
            "url_secret" => {
                reference(text(check, "url_secret")?, &place)?;
                text(check, "path")?;
            }
            _ => {
                slug(text(check, "service")?)?;
                text(check, "path")?;
            }
        }
        if let Some(path) = check.get("path") {
            let path = path
                .as_str()
                .with_context(|| format!("{place}: path must be a string"))?;
            if !path.starts_with('/') {
                bail!("{place}: path {path:?} must start with /");
            }
        }
        if let Some(headers) = check.get("headers") {
            let headers = headers.as_object().with_context(|| {
                format!("{place}: headers must map a header name to role#field")
            })?;
            for (name, value) in headers {
                let value = value
                    .as_str()
                    .with_context(|| format!("{place}: header {name} must name role#field"))?;
                reference(value, &format!("{place}: header {name}"))?;
            }
        }
        // The status is named by its HTTP reason phrase (`OK`, `Unauthorized`),
        // the words a status line carries, so no number stands in the catalog.
        text(check, "expect_status")?;
        if check.get("expect_body").is_some() {
            text(check, "expect_body")?;
        }
    }
    Ok(())
}
