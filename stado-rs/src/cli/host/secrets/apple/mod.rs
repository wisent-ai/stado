//! `stado credentials item apple-profile --host HOST ITEM --profile FIELD=BUNDLE_ID…`:
//! a product's Apple provisioning profiles, made or found through the App
//! Store Connect API with the team's API key, and stored base64 as fields of
//! one item in HOST's owner vault, where the product's release manifest reads
//! them. An active profile of the same name is reused, not duplicated; a
//! bundle id the team never registered is refused by name.

mod api;

use base64::Engine;
use clap::Args;
use serde_json::{json, Map, Value};

use crate::cli::CmdError;

#[derive(Args)]
pub struct AppleProfileArgs {
    /// Host whose owner vault receives the item.
    #[arg(long)]
    host: String,
    /// Item the profiles are stored in (for example tama-desktop-signing).
    item: String,
    /// FIELD=BUNDLE_ID: store the profile for BUNDLE_ID as FIELD. Repeat
    /// for each bundle the product signs.
    #[arg(long = "profile", required = true, value_parser = profile_pair)]
    profiles: Vec<(String, String)>,
    /// Item holding the team API key (fields key_id, issuer_id,
    /// private_key_p8_base64). No item is built in.
    #[arg(long)]
    credentials: String,
    /// Profile type; MAC_APP_DIRECT is a Developer ID profile.
    #[arg(long = "type", default_value = "MAC_APP_DIRECT")]
    profile_type: String,
    /// Certificate type the profile names; DEVELOPER_ID_APPLICATION for
    /// Developer ID.
    #[arg(long, default_value = "DEVELOPER_ID_APPLICATION")]
    certificate_type: String,
    #[arg(long)]
    json: bool,
}

fn profile_pair(value: &str) -> Result<(String, String), String> {
    match value.split_once('=') {
        Some((field, bundle)) if !field.is_empty() && !bundle.is_empty() => {
            Ok((field.to_string(), bundle.to_string()))
        }
        _ => Err(format!("{value:?} is not FIELD=BUNDLE_ID")),
    }
}

async fn credential(item: &str, field: &str) -> Result<String, CmdError> {
    crate::credential_store::read_string(item, field)
        .await
        .map_err(|error| CmdError::click(format!("{item}#{field}: {error}")))?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CmdError::click(format!("{item} holds no {field}")))
}

async fn api_key(item: &str) -> Result<api::ApiKey, CmdError> {
    let encoded = credential(item, "private_key_p8_base64").await?;
    let pem = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|error| CmdError::click(format!("{item}#private_key_p8_base64: {error}")))
        .and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|error| CmdError::click(format!("{item}#private_key_p8_base64: {error}")))
        })?;
    Ok(api::ApiKey {
        key_id: credential(item, "key_id").await?,
        issuer_id: credential(item, "issuer_id").await?,
        private_key_pem: pem,
    })
}

pub async fn apple_profile(args: AppleProfileArgs) -> Result<(), CmdError> {
    let bearer = api::bearer(&api_key(&args.credentials).await?)?;
    let certificates = api::certificates(&bearer, &args.certificate_type).await?;
    let mut fields = Map::new();
    let mut report = Vec::new();
    for (field, bundle_identifier) in &args.profiles {
        let bundle = api::bundle_id(&bearer, bundle_identifier).await?;
        let name = format!("stado {bundle_identifier} {}", args.profile_type);
        let (content, origin) = match api::active_profile(&bearer, &name).await? {
            Some(content) => (content, "reused"),
            None => {
                let created =
                    api::create_profile(&bearer, &name, &args.profile_type, &bundle, &certificates)
                        .await?;
                (created, "created")
            }
        };
        fields.insert(field.clone(), json!(content));
        report.push(json!({"field": field, "bundle_id": bundle_identifier, "profile": name, "origin": origin}));
    }
    let payload = json!({
        "schema": "skarbiec.item.v2",
        "kind": "bundle",
        "fields": Value::Object(fields),
        "context": {"provider": "apple", "profile_type": args.profile_type},
    });
    let written = crate::cli::host::write_vault_item(
        &args.host,
        &args.item,
        "bundle",
        &payload.to_string(),
        false,
        None,
    )
    .await?;
    let outcome =
        json!({"item": args.item, "host": args.host, "profiles": report, "vault": written});
    if args.json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        for row in &report {
            println!(
                "{}: {} profile {} for {}",
                row["field"].as_str().unwrap_or_default(),
                row["origin"].as_str().unwrap_or_default(),
                row["profile"].as_str().unwrap_or_default(),
                row["bundle_id"].as_str().unwrap_or_default()
            );
        }
        println!("stored in {} on {}", args.item, args.host);
    }
    Ok(())
}
