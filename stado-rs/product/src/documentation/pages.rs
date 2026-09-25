use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::LazyLock};
use url::Url;

const MAX_REDIRECTS: usize = 6;
static LINKS: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href]").expect("valid link selector"));
static CANONICAL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("link[rel~=canonical]").expect("valid canonical selector"));

fn fetch(client: &Client, url: &str) -> Result<(u16, String)> {
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("request {url}"))?;
    let status = response.status().as_u16();
    Ok((
        status,
        response
            .text()
            .with_context(|| format!("read response from {url}"))?,
    ))
}

fn verify(client: &Client, origin: &str) -> Result<Value> {
    let base = Url::parse(origin)?;
    let index = format!("{origin}/docs/cli");
    let (status, body) = fetch(client, &index)?;
    if status != 200 {
        bail!("{index} returned {status}");
    }
    let document = Html::parse_document(&body);
    let mut routes = BTreeSet::new();
    for element in document.select(&LINKS) {
        let Some(href) = element.value().attr("href") else {
            continue;
        };
        let Ok(url) = base.join(href) else {
            continue;
        };
        if url.origin() != base.origin() {
            continue;
        }
        let path = url.path().trim_end_matches('/');
        if path.starts_with("/docs/cli/") {
            routes.insert(path.to_owned());
        }
    }
    if routes.is_empty() {
        bail!("CLI index links no command pages");
    }
    let mut failures = Vec::new();
    for route in &routes {
        let url = format!("{origin}{route}");
        match fetch(client, &url) {
            Ok((status, _)) if status != 200 => failures.push(format!("{route} returned {status}")),
            Ok((_, body)) => {
                let document = Html::parse_document(&body);
                let canonical: Vec<_> = document
                    .select(&CANONICAL)
                    .filter_map(|element| element.value().attr("href"))
                    .collect();
                if canonical.len() != 1
                    || (canonical[0] != url && canonical[0] != format!("{url}/"))
                {
                    failures.push(format!("{route} canonical is {canonical:?}"));
                }
            }
            Err(error) => failures.push(format!("{route} request failed: {error:#}")),
        }
    }
    Ok(
        json!({"origin": origin, "ok": failures.is_empty(), "commands": routes.len(), "routes": routes, "failures": failures}),
    )
}

pub fn report(products: &Value, selected: &[String]) -> Result<Value> {
    let origins: BTreeSet<_> = products["products"]
        .as_array()
        .context("CLI catalog has no products")?
        .iter()
        .map(|product| {
            product["docs_origin"]
                .as_str()
                .context("CLI product has no documentation origin")
        })
        .collect::<Result<_>>()?;
    let unmatched: Vec<_> = selected
        .iter()
        .filter(|origin| !origins.contains(origin.as_str()))
        .collect();
    if !unmatched.is_empty() {
        bail!("documentation origins are not present in the CLI catalog: {unmatched:?}");
    }
    if origins.is_empty() {
        bail!("CLI catalog has no documentation origins to verify");
    }
    let client = Client::builder()
        .user_agent("wisent-cli-documentation/1")
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .build()?;
    let mut reports = Vec::new();
    for origin in origins {
        if !selected.is_empty() && !selected.iter().any(|selected| selected == origin) {
            continue;
        }
        reports.push(match verify(&client, origin) {
            Ok(report) => report,
            Err(error) => json!({"origin": origin, "ok": false, "error": format!("{error:#}")}),
        });
    }
    Ok(json!({"ok": reports.iter().all(|report| report["ok"] == true), "products": reports}))
}
