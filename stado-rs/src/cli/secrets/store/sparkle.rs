//! `stado credentials sparkle-key mint PRODUCT --info-plist PATH [--replace]`:
//! mint a desktop product's Sparkle update key.
//!
//! Sparkle signs every update with an Ed25519 key and an installed app
//! accepts only updates signed by the key whose public half its Info.plist
//! carries (`SUPublicEDKey`). A release build reads the pair from the item
//! playing role `<product>-sparkle` (`private_key`, the base64 seed
//! `sign_update --ed-key-file` reads, and `public_key`) and refuses an app
//! whose `SUPublicEDKey` is not that public half. This command generates the
//! pair in this process, stores it under that role and writes the public half
//! into the app's Info.plist, so the key never passes a shell or a person.
//!
//! A role that already holds a key is refused without `--replace`: copies
//! installed with the old key can never take an update signed by a new one.

use base64::Engine;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::json;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::items::{store, Store};

const PUBLIC_KEY_ENTRY: &str = "SUPublicEDKey";

/// The seed of a new Ed25519 key: the random bytes of two version-4 UUIDs,
/// the same source `stado database create` draws a database password from.
fn seed() -> Vec<u8> {
    let mut seed = Vec::new();
    seed.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    seed
}

pub(crate) async fn sparkle_key(
    product: &str,
    info_plist: &std::path::Path,
    replace: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    if !info_plist.is_file() {
        return Err(CmdError::usage(format!(
            "--info-plist {} is not a file; name the Info.plist of {product}'s app, whose {PUBLIC_KEY_ENTRY} must carry the new public key",
            info_plist.display()
        )));
    }
    let role = format!("{product}-sparkle");
    let stored = match store()? {
        Store::Skarbiec(vault) => vault.list_items().await,
        Store::File(path) => crate::credential_store::write::file_items(&path),
    }
    .map_err(CmdError::from)?;
    let holders = crate::skarbiec::roles::holders(&stored, &role);
    let held = match holders.as_slice() {
        [] => None,
        [one] => Some(one.id.clone()),
        several => {
            return Err(CmdError::refused(format!(
                "{} items play role {role}; exactly one may hold {product}'s Sparkle key",
                several.len()
            )))
        }
    };
    if let (Some(item), false) = (&held, replace) {
        return Err(CmdError::refused(format!(
            "{item} already plays role {role}; copies of {product} installed with its key accept only updates it signs, so a new key is written only with --replace"
        )));
    }
    let seed = seed();
    let pair = Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|error| {
        CmdError::click(format!(
            "the Ed25519 key for {product} could not be made: {error}"
        ))
        .stating(FailureCode::Config)
    })?;
    let engine = base64::engine::general_purpose::STANDARD;
    let private_key = engine.encode(&seed);
    let public_key = engine.encode(pair.public_key().as_ref());
    let fields = json!({ "private_key": private_key, "public_key": public_key });
    let context = json!({ "product": product, "purpose": "sparkle update signing" });
    let item =
        crate::credential_store::write::write_role_item_with(&role, "bundle", &fields, &context)
            .await
            .map_err(CmdError::from)?;
    let written = crate::wait::output(std::process::Command::new("/usr/bin/plutil")
        .arg("-replace")
        .arg(PUBLIC_KEY_ENTRY)
        .arg("-string")
        .arg(&public_key)
        .arg(info_plist))
        .map_err(|error| {
            CmdError::click(format!(
                "/usr/bin/plutil could not start, so {} does not carry the key stored as {item}: {error}",
                info_plist.display()
            ))
            .stating(FailureCode::InfraDown)
        })?;
    if !written.status.success() {
        return Err(CmdError::click(format!(
            "{item} holds the new {role} key, but {} was not updated: plutil: {}; set {PUBLIC_KEY_ENTRY} to {public_key}",
            info_plist.display(),
            String::from_utf8_lossy(&written.stderr).trim()
        ))
        .stating(FailureCode::InfraDown));
    }
    if json_output {
        println!(
            "{}",
            json!({ "product": product, "role": role, "item": item, "public_key": public_key, "info_plist": info_plist, "replaced": held })
        );
    } else {
        println!(
            "{item} plays {role}; {} carries {PUBLIC_KEY_ENTRY} {public_key}; commit it so the next release signs with this key",
            info_plist.display()
        );
    }
    Ok(())
}
