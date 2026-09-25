use anyhow::{bail, Context, Result};
use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION},
};
use serde::de::DeserializeOwned;
use std::{env, process::Command};
use url::Url;

pub const REPOSITORIES_PER_PAGE: usize = 100;

pub fn client() -> Result<Client> {
    let token = env::var("GITHUB_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("GH_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
    let token = match token {
        Some(token) => token,
        None => {
            let output = Command::new("gh")
                .args(["auth", "token"])
                .output()
                .context("GITHUB_TOKEN, GH_TOKEN, or authenticated gh is required")?;
            if !output.status.success() {
                bail!(
                    "gh auth token failed ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            String::from_utf8(output.stdout)?
        }
    };
    if token.trim().is_empty() {
        bail!("GitHub authentication returned an empty token");
    }
    let mut authorization = HeaderValue::from_str(&format!("Bearer {}", token.trim()))?;
    authorization.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, authorization);
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        "x-github-api-version",
        HeaderValue::from_static("2022-11-28"),
    );
    Ok(Client::builder()
        .default_headers(headers)
        .user_agent("wisent-markdown-policy")
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

pub fn url(segments: &[&str]) -> Result<Url> {
    let mut url = Url::parse("https://api.github.com")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("GitHub API has no hierarchical path"))?
        .extend(segments);
    Ok(url)
}

pub fn request<T: DeserializeOwned>(client: &Client, url: Url) -> Result<T> {
    let response = client
        .get(url.clone())
        .send()
        .with_context(|| format!("GitHub API request {url}"))?;
    let status = response.status();
    if !status.is_success() {
        let detail = response
            .text()
            .with_context(|| format!("read GitHub API {status} for {url}"))?;
        bail!("GitHub API {status} for {url}: {detail}");
    }
    response
        .json()
        .with_context(|| format!("decode GitHub API response for {url}"))
}
