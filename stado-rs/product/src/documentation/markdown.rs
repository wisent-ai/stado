use super::github::{self, REPOSITORIES_PER_PAGE};
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Repository {
    name: String,
    full_name: String,
    archived: bool,
    default_branch: Option<String>,
}
#[derive(Deserialize)]
struct Entry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
}
#[derive(Deserialize)]
struct Tree {
    truncated: bool,
    tree: Vec<Entry>,
}

fn paths(client: &Client, repository: &Repository) -> Result<Vec<String>> {
    let Some(branch) = &repository.default_branch else {
        return Ok(Vec::new());
    };
    let (owner, name) = repository
        .full_name
        .split_once('/')
        .context("GitHub repository has no owner/name identity")?;
    let mut url = github::url(&["repos", owner, name, "git", "trees", branch])?;
    url.query_pairs_mut().append_pair("recursive", "1");
    let tree: Tree = github::request(client, url)?;
    if tree.truncated {
        bail!(
            "recursive Git tree was truncated for {}",
            repository.full_name
        );
    }
    let mut forbidden: Vec<_> = tree
        .tree
        .into_iter()
        .filter(|entry| entry.kind == "blob" && entry.path != "README.md")
        .filter(|entry| {
            let path = entry.path.to_ascii_lowercase();
            path.ends_with(".md") || path.ends_with(".markdown")
        })
        .map(|entry| entry.path)
        .collect();
    forbidden.sort();
    Ok(forbidden)
}

pub fn report(organization: &str, include_archived: bool) -> Result<Value> {
    let client = github::client()?;
    let mut page = 1;
    let mut scanned = 0;
    let mut violations = BTreeMap::new();
    let mut failures = Vec::new();
    loop {
        let mut url = github::url(&["orgs", organization, "repos"])?;
        url.query_pairs_mut()
            .append_pair("type", "all")
            .append_pair("per_page", &REPOSITORIES_PER_PAGE.to_string())
            .append_pair("page", &page.to_string());
        let repositories: Vec<Repository> = github::request(&client, url)?;
        let count = repositories.len();
        for repository in repositories {
            if repository.archived && !include_archived {
                continue;
            }
            scanned += 1;
            match paths(&client, &repository) {
                Ok(paths) if !paths.is_empty() => {
                    violations.insert(repository.name, paths);
                }
                Ok(_) => {}
                Err(error) => failures.push(
                    json!({"repository": repository.full_name, "error": format!("{error:#}")}),
                ),
            }
        }
        if count < REPOSITORIES_PER_PAGE {
            break;
        }
        page += 1;
    }
    Ok(
        json!({"organization": organization, "repositories": scanned, "ok": violations.is_empty() && failures.is_empty(),
        "violations": violations, "failures": failures}),
    )
}
