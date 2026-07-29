//! `stado capabilities` — discover user-facing features and provider support.

use serde::Serialize;

use super::CmdError;
use crate::capabilities::{self, ProductCapability};

#[derive(Serialize)]
struct CatalogDocument<'a> {
    schema: &'static str,
    capabilities: Vec<&'a ProductCapability>,
    control_config: &'static [crate::capabilities::ConfigField],
}

pub fn run(filter: Option<&str>, as_json: bool) -> Result<(), CmdError> {
    let selected = match filter {
        Some(id) => vec![capabilities::product_capability(id).ok_or_else(|| {
            let choices = capabilities::product_capabilities()
                .iter()
                .map(|capability| capability.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            CmdError::usage(format!("unknown capability {id:?}; use one of: {choices}"))
        })?],
        None => capabilities::product_capabilities().iter().collect(),
    };

    if as_json {
        let document = CatalogDocument {
            schema: "product-v1",
            capabilities: selected,
            control_config: capabilities::CONTROL_CONFIG,
        };
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }

    let rows = selected
        .into_iter()
        .flat_map(|capability| {
            capability.providers.iter().map(move |provider| {
                vec![
                    capability.id.as_str().to_string(),
                    provider.provider.as_str().to_string(),
                    provider.support.as_str().to_string(),
                    if provider.implementation.is_empty() {
                        "-".to_string()
                    } else {
                        provider.implementation.to_string()
                    },
                    provider.note.to_string(),
                ]
            })
        })
        .collect::<Vec<_>>();
    super::table::print(
        &[
            "CAPABILITY",
            "PROVIDER",
            "SUPPORT",
            "IMPLEMENTATION",
            "NOTE",
        ],
        &rows,
    );
    Ok(())
}
