//! What each bucket verb does to the host's declaration and the vault's key,
//! and the answer it prints. A write is confirmed by reading the host's
//! configuration back, so exit 0 means the host's own Stado resolves the
//! change, not that a command was sent.

use serde_json::{json, Value};

use crate::cli::host::{remote_config_output, RemoteConfigAction};
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::declared::{
    answer, bucket_name, buckets, declaration_key, declare, described, described_text,
    prefix_policies, read_buckets, reload, store_key, Declared, FIELD,
};
use super::{BucketDeleteArgs, BucketGrantArgs, BucketNameArgs, BucketRotateArgs, BucketTarget};

fn served(host: &str, names: impl Iterator<Item = String>) -> String {
    let names: Vec<String> = names.collect();
    if names.is_empty() {
        format!("{host} serves no bucket")
    } else {
        format!("{host} serves: {}", names.join(", "))
    }
}

pub(super) async fn create(args: &BucketGrantArgs) -> Result<(), CmdError> {
    let name = bucket_name(&args.name)?;
    let host = &args.target.host;
    let policies = prefix_policies(&args.grants)?;
    // A host that declares no bucket yet answers with its refusal of an empty
    // declaration; the first bucket is what ends it.
    if let Declared::Buckets(existing) = read_buckets(host).await? {
        if existing.contains_key(&name) {
            return Err(CmdError::refused(format!(
                "{host} already serves bucket {name}; `stado storage bucket update` changes its \
                 grant and `rotate` its key"
            )));
        }
    }
    let item = crate::config::namespace_item(&name);
    store_key(&item).await?;
    if let Err(error) = declare(&name, host, &policies).await {
        crate::credential_store::write::delete_item_with(&item)
            .await
            .map_err(|cleanup| {
                CmdError::click(format!(
                    "{error}; and vault item {item}, written for it, could not be removed: \
                     {cleanup}"
                ))
            })?;
        return Err(error);
    }
    let reloaded = reload(args.reload_service.as_deref(), host).await?;
    let value = described(&name, host, &policies, &reloaded);
    answer(
        &value,
        args.target.json,
        &described_text(&name, host, &policies),
    )
}

pub(super) async fn list(args: &BucketTarget) -> Result<(), CmdError> {
    let buckets = buckets(&args.host).await?;
    let rows: Vec<Value> = buckets
        .iter()
        .map(|(name, entry)| described(name, &args.host, &entry["prefix_policies"], &Value::Null))
        .collect();
    let text: Vec<String> = buckets
        .iter()
        .map(|(name, entry)| format!("{name}\t{}", entry["prefix_policies"]))
        .collect();
    answer(
        &json!({"host": args.host, "buckets": rows}),
        args.json,
        &text,
    )
}

pub(super) async fn show(args: &BucketNameArgs) -> Result<(), CmdError> {
    let name = bucket_name(&args.name)?;
    let host = &args.target.host;
    let buckets = buckets(host).await?;
    let Some(entry) = buckets.get(&name) else {
        return Err(CmdError::click(format!(
            "no bucket {name}: {}",
            served(host, buckets.keys().cloned())
        ))
        .stating(FailureCode::NotFound));
    };
    let grant = &entry["prefix_policies"];
    let value = described(&name, host, grant, &Value::Null);
    answer(
        &value,
        args.target.json,
        &described_text(&name, host, grant),
    )
}

pub(super) async fn update(args: &BucketGrantArgs) -> Result<(), CmdError> {
    let name = bucket_name(&args.name)?;
    let host = &args.target.host;
    let policies = prefix_policies(&args.grants)?;
    let existing = buckets(host).await?;
    if !existing.contains_key(&name) {
        return Err(CmdError::click(format!(
            "no bucket {name}: {}; `stado storage bucket create` declares it",
            served(host, existing.keys().cloned())
        ))
        .stating(FailureCode::NotFound));
    }
    declare(&name, host, &policies).await?;
    let reloaded = reload(args.reload_service.as_deref(), host).await?;
    let value = described(&name, host, &policies, &reloaded);
    answer(
        &value,
        args.target.json,
        &described_text(&name, host, &policies),
    )
}

pub(super) async fn rotate(args: &BucketRotateArgs) -> Result<(), CmdError> {
    let name = bucket_name(&args.name)?;
    let item = crate::config::namespace_item(&name);
    let held = crate::credential_store::read_declared_string(&item, FIELD)
        .await
        .map_err(|error| CmdError::click(format!("reading vault item {item} failed: {error}")))?;
    if held.is_none() {
        return Err(CmdError::click(format!(
            "the vault holds no key for bucket {name} (item {item}, field {FIELD}); `stado \
             storage bucket create` mints it"
        ))
        .stating(FailureCode::NotFound));
    }
    store_key(&item).await?;
    let value = json!({"bucket": name, "key": {"item": item, "field": FIELD}, "rotated": true});
    answer(
        &value,
        args.json,
        &[format!(
            "bucket {name}: vault item {item} field {FIELD} holds a new key; the old one is \
             refused from the next request"
        )],
    )
}

pub(super) async fn delete(args: &BucketDeleteArgs) -> Result<(), CmdError> {
    let name = bucket_name(&args.name)?;
    if args.confirm != name {
        return Err(CmdError::usage(format!(
            "--confirm must repeat the bucket name {name:?}; nothing was removed"
        )));
    }
    let host = &args.target.host;
    let existing = buckets(host).await?;
    if !existing.contains_key(&name) {
        return Err(CmdError::click(format!(
            "no bucket {name}: {}",
            served(host, existing.keys().cloned())
        ))
        .stating(FailureCode::NotFound));
    }
    let target = crate::cli::canonical_host(host).await?;
    let key = declaration_key(&name);
    remote_config_output(
        &target,
        RemoteConfigAction::Unset { key: &key },
        &crate::deploy::production_runner(),
    )
    .await?;
    let item = crate::config::namespace_item(&name);
    crate::credential_store::write::delete_item_with(&item)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "{host} no longer declares bucket {name}, but vault item {item} could not be \
                 removed: {error}"
            ))
        })?;
    let reloaded = reload(args.reload_service.as_deref(), host).await?;
    let value = json!({
        "bucket": name,
        "host": host,
        "removed": {"declaration": key, "item": item},
        "objects_kept": format!("ecosystem/{name}/"),
        "reloaded": reloaded,
    });
    answer(
        &value,
        args.target.json,
        &[format!(
            "bucket {name} removed from {host}; its key item {item} is in the vault's trash; its \
             objects under ecosystem/{name}/ stay in the host's store"
        )],
    )
}
