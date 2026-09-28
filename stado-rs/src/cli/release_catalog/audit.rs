//! `stado release catalog audit`: every declared publisher checked against
//! the release catalog, without contacting repository hosts.

use std::collections::BTreeSet;

use crate::release_pipeline::{self, ReleaseCatalogEntry};

use super::catalog_uri;
use crate::cli::CmdError;

/// The human-readable form of one audit: the tally on stdout, then every
/// refusal on stderr, so a shell pipeline keeps the tally alone.
fn print_audit(entries: &[ReleaseCatalogEntry], failures: &[String]) {
    println!(
        "catalog products={} failures={}",
        entries.len(),
        failures.len()
    );
    for failure in failures {
        eprintln!("catalog refusal: {failure}");
    }
}

pub(super) async fn audit(json: bool) -> Result<(), CmdError> {
    let publishers = crate::config::release_api_publishers().map_err(|problems| {
        CmdError::click(format!(
            "release catalog audit refused invalid release_api.publishers: {}",
            problems.join("; ")
        ))
    })?;
    let mut products = BTreeSet::new();
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for product in publishers.keys() {
        let uri = catalog_uri(product);
        // A publisher declared for a product the catalog never received is a
        // product nothing builds: say which, and the command that registers
        // it or withdraws it, instead of the object store's bare 404.
        if matches!(
            crate::cli::storage::fetch_object_versioned(&uri).await,
            Ok(None)
        ) {
            failures.push(format!(
                "{product}: release_api.publishers declares it but the release catalog holds no \
                 entry for it, so nothing builds it; register its checkout with `stado release \
                 catalog enroll <checkout>`, or for a retired product withdraw the declaration \
                 with `stado release catalog withdraw-publisher {product}`"
            ));
            continue;
        }
        match crate::cli::storage::fetch_object(&uri)
            .await
            .and_then(|bytes| {
                let entry: ReleaseCatalogEntry = serde_json::from_slice(&bytes)?;
                release_pipeline::validate_catalog_entry(&entry).map_err(CmdError::click)?;
                if uri != catalog_uri(&entry.product) {
                    return Err(CmdError::click(
                        "catalog entry product disagrees with object coordinate",
                    ));
                }
                Ok(entry)
            }) {
            Ok(entry) if products.insert(entry.product.clone()) => entries.push(entry),
            Ok(entry) => failures.push(format!("duplicate catalog product {}", entry.product)),
            Err(error) => failures.push(format!("{uri}: {error}")),
        }
    }
    if entries.is_empty() {
        failures.push("release catalog is silent: it contains no explicit product entries".into());
    }
    let report = serde_json::json!({
        "catalog": "stado://system/release-catalog/",
        "products": entries,
        "failures": failures,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_audit(&entries, &failures);
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CmdError::click(
            "release catalog audit refused malformed, duplicate, or silent entries",
        ))
    }
}
