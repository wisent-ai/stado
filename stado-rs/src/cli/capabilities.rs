//! `stado capabilities` — discover the central capability/variant catalog.

use serde::Serialize;

use super::CmdError;
use crate::capabilities::{self, Capability, CapabilityKind, CapabilityVariant};

#[derive(Serialize)]
struct CatalogDocument<'a> {
    schema_version: u8,
    capabilities: Vec<CapabilityView<'a>>,
}

#[derive(Serialize)]
struct CapabilityView<'a> {
    id: &'static str,
    selection_mode: &'static str,
    summary: &'static str,
    variants: Vec<VariantView<'a>>,
}

#[derive(Serialize)]
struct VariantView<'a> {
    id: &'static str,
    aliases: &'a [&'static str],
    provider: Option<&'static str>,
    implementation: &'static str,
    summary: &'static str,
    configurable: bool,
    constructible: bool,
    state: &'static str,
}

pub fn run(filter: Option<&str>, as_json: bool) -> Result<(), CmdError> {
    let selected: Vec<&Capability> = match filter {
        Some(id) => vec![capabilities::get(id).ok_or_else(|| {
            let choices = capabilities::all()
                .iter()
                .map(|capability| capability.kind.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            CmdError::usage(format!("unknown capability {id:?}; use one of: {choices}"))
        })?],
        None => capabilities::all().iter().collect(),
    };

    if as_json {
        let document = CatalogDocument {
            schema_version: 1,
            capabilities: selected.into_iter().map(view).collect(),
        };
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }

    let rows = selected
        .into_iter()
        .flat_map(|capability| {
            capability.variants.iter().map(move |variant| {
                vec![
                    capability.kind.as_str().to_string(),
                    variant.id.to_string(),
                    variant.provider.unwrap_or("-").to_string(),
                    capability.selection.as_str().to_string(),
                    state(capability.kind, variant).to_string(),
                    variant.implementation.to_string(),
                ]
            })
        })
        .collect::<Vec<_>>();
    super::table::print(
        &[
            "CAPABILITY",
            "VARIANT",
            "PROVIDER",
            "SELECTION",
            "STATE",
            "IMPLEMENTATION",
        ],
        &rows,
    );
    Ok(())
}

fn view(capability: &Capability) -> CapabilityView<'_> {
    CapabilityView {
        id: capability.kind.as_str(),
        selection_mode: capability.selection.as_str(),
        summary: capability.summary,
        variants: capability
            .variants
            .iter()
            .map(|variant| VariantView {
                id: variant.id,
                aliases: variant.aliases,
                provider: variant.provider,
                implementation: variant.implementation,
                summary: variant.summary,
                configurable: variant.configurable,
                constructible: variant.constructible,
                state: state(capability.kind, variant),
            })
            .collect(),
    }
}

fn state(kind: CapabilityKind, variant: &CapabilityVariant) -> &'static str {
    match kind {
        CapabilityKind::Compute => {
            if contains_variant(
                crate::config::wc_disabled_providers(),
                CapabilityKind::Compute,
                variant.id,
            ) {
                "disabled"
            } else if contains_variant(
                crate::config::wc_providers(),
                CapabilityKind::Compute,
                variant.id,
            ) {
                "active"
            } else if variant.configurable {
                "available"
            } else {
                "built-in"
            }
        }
        CapabilityKind::Storage => {
            let primary = same_variant(
                CapabilityKind::Storage,
                crate::config::wc_storage_backend(),
                variant.id,
            );
            let backup = same_variant(
                CapabilityKind::Storage,
                crate::config::wc_backup_storage_backend(),
                variant.id,
            );
            match (primary, backup) {
                (true, true) => "primary+backup",
                (true, false) => "primary",
                (false, true) => "backup",
                (false, false) => "available",
            }
        }
        _ => "built-in",
    }
}

fn contains_variant(names: &[String], kind: CapabilityKind, expected: &str) -> bool {
    names.iter().any(|name| same_variant(kind, name, expected))
}

fn same_variant(kind: CapabilityKind, actual: &str, expected: &str) -> bool {
    capabilities::variant(kind, actual).is_some_and(|variant| variant.id == expected)
}
