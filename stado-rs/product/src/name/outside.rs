//! What outside the catalog still carries a renamed product's former id.
//!
//! Each of these is owned by another system: a repository's name by GitHub,
//! a release manifest's `product` by the product's repository, a site's
//! address by the site. `name set` changes none of them; it reads each one
//! and names it with what moves it, so the rename is finished by the owner
//! of each place rather than half done by this command. Nothing here
//! changes state.

use crate::common::Runtime;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;

/// Whether repository name `short` is named for product id `id`: the id
/// itself, or the id and a surface suffix (`<id>-desktop`).
fn named_for(short: &str, id: &str) -> bool {
    short == id || short.starts_with(&format!("{id}-"))
}

/// Every place outside the catalog that still carries `former`, the former
/// id of `product`, now `id`. `org`, when given, also reads the
/// organization's repositories for ones named for the former id that no
/// record claims (a landing site, a client).
pub(super) fn read(
    runtime: &Runtime,
    product: &Value,
    former: &str,
    id: &str,
    org: Option<&str>,
) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    let claimed = crate::dependencies::repositories(product);
    for repository in &claimed {
        let Some((owner, short)) = repository.split_once('/') else {
            bail!("{id} names repository {repository:?}, which is not OWNER/NAME");
        };
        rows.extend(manifests(runtime, product, repository, short, former)?);
        if !named_for(short, former) {
            continue;
        }
        let renamed = format!("{id}{}", &short[former.len()..]);
        rows.push(json!({
            "what": format!("repository {repository}"),
            "state": "named for the former id",
            "next": format!(
                "`gh repo rename {renamed} --repo {repository}`, then record \
                 {owner}/{renamed} in {id}'s record with `stado product registry set {id}` \
                 (--owner-repository, --add-surface KIND={owner}/{renamed})"
            ),
        }));
    }
    if let Some(org) = org {
        for repository in crate::catalog::organization_repositories(org)? {
            let Some(full) = repository["nameWithOwner"].as_str() else {
                bail!("`gh repo list {org}` gave a repository no nameWithOwner: {repository}");
            };
            let Some((_, short)) = full.split_once('/') else {
                bail!("`gh repo list {org}` gave {full}, which is not OWNER/NAME");
            };
            if repository["isArchived"] == true || claimed.contains(full) || !named_for(short, former) {
                continue;
            }
            rows.push(json!({
                "what": format!("repository {full}"),
                "state": "named for the former id and claimed by no product",
                "next": format!(
                    "rename it for {id} with `gh repo rename`, or claim it for {id} with `stado \
                     product registry set {id} --add-surface KIND={full}`"
                ),
            }));
        }
    }
    for surface in product["surfaces"].as_array().into_iter().flatten() {
        if surface["kind"] != "cli" {
            continue;
        }
        let origin = crate::catalog::docs_origin(surface, id);
        if origin == format!("https://{former}.wisent.com") {
            rows.push(json!({
                "what": format!("documentation site {origin}"),
                "state": "addressed by the former id",
                "next": format!(
                    "publish the site at https://{id}.wisent.com, then drop docs_origin from \
                     {id}'s cli surface (`stado product registry set {id} --add-surface \
                     cli=<repository>`)"
                ),
            }));
        }
    }
    Ok(rows)
}

/// The release manifests `product`'s installations read from `repository`
/// whose `product` is still `former`, read from the repository's canonical
/// checkout; a checkout the workspace does not hold is said, not skipped.
fn manifests(
    runtime: &Runtime,
    product: &Value,
    repository: &str,
    short: &str,
    former: &str,
) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    let mut read = std::collections::BTreeSet::new();
    let checkout = runtime.workspace.join(short);
    for installation in product["installations"].as_array().into_iter().flatten() {
        if installation["repository"] != repository {
            continue;
        }
        let Some(manifest) = installation["manifest"].as_str() else {
            continue;
        };
        if !read.insert(manifest) {
            continue;
        }
        let path = checkout.join(manifest);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                rows.push(json!({
                    "what": format!("release manifest {}", path.display()),
                    "state": "not read: the workspace holds no such file",
                    "next": format!("check {manifest} in {repository} by hand, or clone it into {}", runtime.workspace.display()),
                }));
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        let declared: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not JSON", path.display()))?;
        if declared["product"] != former {
            continue;
        }
        let surface = product["installations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|other| other["repository"] == repository && other["manifest"] == manifest)
            .filter_map(|other| other["surface"].as_str())
            .collect::<Vec<_>>()
            .join(", ");
        rows.push(json!({
            "what": format!("release manifest {}", path.display()),
            "state": "declares the former id as its product",
            "next": format!(
                "the catalog keeps release_product = {former} on the installations of {surface}, so \
                 installs still match it; publish under the new id by changing the manifest's \
                 product and removing release_product in one change"
            ),
        }));
    }
    Ok(rows)
}
