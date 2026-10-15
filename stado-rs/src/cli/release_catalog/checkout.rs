use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cli::release_newest::declaring_checkouts;
use crate::cli::CmdError;
use crate::release_control;
use crate::release_pipeline::{self, PRODUCT_MANIFEST};

use super::{product, publish_entry};

pub(super) async fn sync(root: &Path, json: bool) -> Result<(), CmdError> {
    let root = root.canonicalize()?;
    let paths: Vec<PathBuf> = declaring_checkouts(&root)?
        .into_iter()
        .map(|checkout| checkout.join(PRODUCT_MANIFEST))
        .collect();
    let mut declarations = BTreeMap::new();
    for path in paths {
        let bytes = std::fs::read(&path)?;
        let manifest =
            release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::declaration)?;
        let name = product(&manifest).to_string();
        if declarations
            .insert(name.clone(), (manifest, bytes))
            .is_some()
        {
            return Err(
                CmdError::click(format!("catalog sync found duplicate product {name:?}"))
                    .stating(crate::primitives::failure::FailureCode::Config),
            );
        }
    }
    if declarations.is_empty() {
        return Err(
            CmdError::click(format!("{} contains no {PRODUCT_MANIFEST}", root.display()))
                .stating(crate::primitives::failure::FailureCode::NotFound),
        );
    }
    let mut entries = Vec::new();
    for (name, (manifest, bytes)) in declarations {
        // The catalog entry is written with the product's own publisher
        // bearer. A product this host declares no publisher for answers
        // Skarbiec's 403 with a hint to rebind Stado's grant, which is not the
        // cause; `enroll`, `build submit` and `release submit` declare it
        // first, and so does this.
        super::publisher::ensure_publisher(&name).await?;
        entries.push(publish_entry(manifest, release_control::sha256_bytes(&bytes), None).await?);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for entry in entries {
            println!(
                "cataloged {} manifest={}",
                entry.product, entry.manifest_sha256
            );
        }
    }
    Ok(())
}
