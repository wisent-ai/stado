//! `stado registry validate PATH` and `stado registry import PATH` — the two
//! verbs that read an operator's own registry-v2 file.

use std::path::PathBuf;

use crate::cli::registry::commands::source_path;
use crate::cli::registry::write::conflict::REGISTRY_CONFLICT_EXIT;
use crate::cli::CmdError;
use crate::targets::validate_registry_file;

pub fn validate(path: Option<String>) -> Result<(), CmdError> {
    let source = source_path(path);
    validate_registry_file(&source).map_err(|exc| CmdError::click(exc.to_string()))?;
    println!("valid registry: {}", source.display());
    Ok(())
}
fn import_names(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}

fn render_import_receipt(receipt: &crate::registry_import::RegistryImportReceipt) {
    println!(
        "registry import: {}{}",
        receipt.state,
        receipt
            .generation
            .as_deref()
            .map(|generation| format!(" (generation {generation})"))
            .unwrap_or_default()
    );
    println!(
        "  imported hosts: {}",
        import_names(&receipt.imported_targets)
    );
    println!(
        "  unchanged hosts: {}",
        import_names(&receipt.unchanged_targets)
    );
    println!(
        "  imported fleets: {}",
        import_names(&receipt.imported_fleets)
    );
    println!(
        "  unchanged fleets: {}",
        import_names(&receipt.unchanged_fleets)
    );
    println!(
        "  imported sections: {}",
        import_names(&receipt.imported_sections)
    );
    for conflict in &receipt.conflicts {
        println!("  conflict: {}: {}", conflict.path, conflict.reason);
    }
    for rejection in &receipt.rejected {
        println!("  rejected: {rejection}");
    }
}

/// Additively adopt an existing registry-v2 file into the canonical registry.
///
/// Both this command and `POST /api/registry/import` call
/// [`crate::registry_import::import_bytes`]. The operation validates the whole
/// source before opening the destination, preserves every destination-only
/// field, refuses differing records, and verifies the conditional write before
/// returning an accepted receipt.
pub async fn import(path: String, json_output: bool) -> Result<(), CmdError> {
    let source = PathBuf::from(&path);
    let bytes = std::fs::read(&source).map_err(|error| {
        CmdError::click(format!(
            "cannot read registry import {}: {error}",
            source.display()
        ))
    })?;
    let receipt = crate::registry_import::import_bytes(&bytes)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
    } else {
        render_import_receipt(&receipt);
    }
    if receipt.accepted() {
        crate::cli::setup::onboarding::record_registry_import_accepted(&receipt);
        return Ok(());
    }
    let detail = receipt
        .rejected
        .first()
        .cloned()
        .or_else(|| {
            receipt
                .conflicts
                .first()
                .map(|conflict| format!("{}: {}", conflict.path, conflict.reason))
        })
        .unwrap_or_else(|| "the source was not accepted".to_string());
    Err(CmdError {
        message: Some(format!("registry import {}: {detail}", receipt.state)),
        code: if receipt.state == "conflict" {
            REGISTRY_CONFLICT_EXIT
        } else {
            crate::cli::CLICK_ERROR_CODE
        },
        ..CmdError::default()
    })
}
