//! `stado product documentation star-repositories`: whether the repository
//! each product's in-app GitHub star request links to (`star_repository` in
//! the catalog) is one a stranger can open and star. A repository GitHub
//! answers as private, archived or missing sends the person nowhere, so it is
//! a violation naming the product, the repository and GitHub's answer.

use super::github;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct Repository {
    full_name: String,
    private: bool,
    archived: bool,
}

pub fn report(catalog: &Value) -> Result<Value> {
    let client = github::client()?;
    let products = catalog["products"]
        .as_array()
        .context("the catalog's products must be a list")?;
    let mut checked = Vec::new();
    let mut violations = Vec::new();
    for product in products {
        let id = crate::catalog::text(product, "id")?;
        // A record that names no repository has no star request to open:
        // that product alone is the violation, and every other is checked.
        let Some(repository) = product.get("star_repository") else {
            let row = json!({"product": id, "star_repository": null, "ok": false,
                "cause": format!("{id} names no star_repository, so its GitHub star request links nowhere")});
            violations.push(row.clone());
            checked.push(row);
            continue;
        };
        let repository = repository
            .as_str()
            .with_context(|| format!("{id}.star_repository: expected a string"))?;
        let (owner, name) = repository
            .split_once('/')
            .with_context(|| format!("{id}.star_repository {repository:?} is not OWNER/REPO"))?;
        let answer = github::request::<Repository>(&client, github::url(&["repos", owner, name])?);
        let row = match answer {
            Ok(found) if found.private => {
                json!({"product": id, "star_repository": repository, "ok": false,
                    "cause": format!("GitHub answers {} as private, so a stranger opening the star request gets a 404", found.full_name)})
            }
            Ok(found) if found.archived => {
                json!({"product": id, "star_repository": repository, "ok": false,
                    "cause": format!("GitHub answers {} as archived, so it cannot be starred", found.full_name)})
            }
            Ok(found) => json!({"product": id, "star_repository": found.full_name, "ok": true}),
            Err(error) => json!({"product": id, "star_repository": repository, "ok": false,
                "cause": format!("GitHub did not answer the repository: {error:#}")}),
        };
        if row["ok"] != true {
            violations.push(row.clone());
        }
        checked.push(row);
    }
    Ok(json!({
        "ok": violations.is_empty(),
        "checked": checked.len(),
        "products": checked,
        "violations": violations,
        "repair": "stado product registry set PRODUCT --star-repository OWNER/REPO naming a public repository",
    }))
}
