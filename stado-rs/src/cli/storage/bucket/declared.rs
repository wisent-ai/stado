//! A bucket's two halves and how each is read and written: the declaration
//! in a host's `object_api.namespaces`, read back from the host's own Stado,
//! and the key in the vault item the namespace is bound to.

use serde_json::{json, Map, Value};

use crate::cli::host::{remote_config_output, write_host_config, RemoteConfigAction};
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::remote::object_store::ObjectRef;

/// The field of the key item the bearer, and so the S3 secret, is kept in.
pub(super) const FIELD: &str = "token";
/// How many random bytes a key holds: one SHA-256 output, the size of the
/// HMAC-SHA256 key SigV4 derives every signing key from.
const KEY_BYTES: usize = 32;

pub(super) fn declaration_key(name: &str) -> String {
    format!("object_api.namespaces.{name}")
}

/// A bucket name: a product namespace, never a release namespace.
pub(super) fn bucket_name(raw: &str) -> Result<String, CmdError> {
    let sentinel =
        ObjectRef::new(raw, "sentinel").map_err(|error| CmdError::usage(error.to_string()))?;
    if sentinel.release_namespace().is_some() {
        return Err(CmdError::usage(format!(
            "{raw} is a release namespace; release objects are published with stado storage put \
             and have no bucket"
        )));
    }
    Ok(sentinel.namespace().to_string())
}

/// The prefix policies `--grant PREFIX=ACTION,ACTION` names.
pub(super) fn prefix_policies(grants: &[String]) -> Result<Value, CmdError> {
    let mut policies = Vec::with_capacity(grants.len());
    for grant in grants {
        let Some((prefix, actions)) = grant.split_once('=') else {
            return Err(CmdError::usage(format!(
                "--grant {grant:?} is not PREFIX=ACTION,ACTION"
            )));
        };
        let actions: Vec<&str> = actions
            .split(',')
            .map(str::trim)
            .filter(|action| !action.is_empty())
            .collect();
        if prefix.trim().is_empty() || actions.is_empty() {
            return Err(CmdError::usage(format!(
                "--grant {grant:?} names no prefix or no action"
            )));
        }
        policies.push(json!({"prefix": prefix.trim(), "actions": actions}));
    }
    Ok(Value::Array(policies))
}

/// What a host's own Stado resolves for its buckets.
pub(super) enum Declared {
    Buckets(Map<String, Value>),
    Refused(Vec<String>),
}

pub(super) async fn read_buckets(host: &str) -> Result<Declared, CmdError> {
    let target = crate::cli::canonical_host(host).await?;
    let shown = remote_config_output(
        &target,
        RemoteConfigAction::Show,
        &crate::deploy::production_runner(),
    )
    .await?;
    let document: Value = serde_json::from_str(&shown).map_err(|error| {
        CmdError::click(format!(
            "{host}: stado config show did not answer JSON: {error}"
        ))
        .stating(FailureCode::InfraDown)
    })?;
    let Some(Value::Object(resolved)) = document.pointer("/resolved/object_api_namespaces") else {
        return Err(CmdError::click(format!(
            "{host}: stado config show reports no object_api_namespaces; its Stado predates \
             buckets"
        ))
        .stating(FailureCode::InfraDown));
    };
    match resolved.get("errors") {
        Some(Value::Array(errors)) => Ok(Declared::Refused(
            errors
                .iter()
                .map(|error| match error.as_str() {
                    Some(text) => text.to_string(),
                    None => error.to_string(),
                })
                .collect(),
        )),
        _ => Ok(Declared::Buckets(resolved.clone())),
    }
}

/// The buckets `host` serves, refused when its declaration is.
pub(super) async fn buckets(host: &str) -> Result<Map<String, Value>, CmdError> {
    match read_buckets(host).await? {
        Declared::Buckets(buckets) => Ok(buckets),
        Declared::Refused(errors) => Err(CmdError::click(format!(
            "{host} declares no readable buckets: object_api.namespaces is refused: {}",
            errors.join("; ")
        ))
        .stating(FailureCode::Config)),
    }
}

fn mint_key() -> Result<String, CmdError> {
    let mut bytes = [0u8; KEY_BYTES];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).map_err(
        |error| {
            CmdError::click(format!(
                "the operating system's random source refused: {error}"
            ))
        },
    )?;
    Ok(hex::encode(bytes))
}

/// Write a fresh key into `item` and make it readable by the identity Stado
/// verifies requests with.
pub(super) async fn store_key(item: &str) -> Result<(), CmdError> {
    let key = mint_key()?;
    crate::credential_store::write::write_item_with(
        item,
        "stado-secret",
        &json!({FIELD: key}),
        &json!({}),
    )
    .await
    .map_err(|error| CmdError::click(format!("writing vault item {item} failed: {error}")))?;
    crate::credential_store::grant::settle_field_reads(item, &[FIELD]).map_err(|error| {
        CmdError::click(format!(
            "vault item {item} was written, but granting Stado's verifier a read of its {FIELD} \
             failed: {error}"
        ))
    })?;
    Ok(())
}

/// Write the declaration and confirm the host's own Stado resolves it.
pub(super) async fn declare(name: &str, host: &str, policies: &Value) -> Result<(), CmdError> {
    let item = crate::config::namespace_item(name);
    let entry = json!({"item": item, "prefix_policies": policies});
    let readback = write_host_config(host, &declaration_key(name), &entry.to_string()).await?;
    let document: Value = serde_json::from_str(&readback).map_err(|error| {
        CmdError::click(format!(
            "{host}: the configuration readback is not JSON: {error}"
        ))
        .stating(FailureCode::InfraDown)
    })?;
    if document
        .pointer(&format!("/resolved/object_api_namespaces/{name}"))
        .is_none()
    {
        return Err(CmdError::click(format!(
            "{host} accepted object_api.namespaces.{name} but its Stado does not resolve it; read \
             `stado host config show {host}` for the namespaces it refuses"
        ))
        .stating(FailureCode::Config));
    }
    Ok(())
}

/// Restart `unit` on `host` when one is named, and say which was.
pub(super) async fn reload(unit: Option<&str>, host: &str) -> Result<Value, CmdError> {
    match unit {
        Some(unit) => {
            crate::cli::service::reconcile_after_config_change(unit, host).await?;
            Ok(json!(unit))
        }
        None => Ok(Value::Null),
    }
}

/// One bucket as every verb reports it.
pub(super) fn described(name: &str, host: &str, grant: &Value, reloaded: &Value) -> Value {
    json!({
        "bucket": name,
        "host": host,
        "grant": grant,
        "key": {"item": crate::config::namespace_item(name), "field": FIELD},
        "s3": {
            "path": format!("{}/{name}", crate::remote::sigv4::S3_ROOT),
            "access_key_id": name,
            "region": crate::remote::sigv4::STADO_REGION,
            "addressing": "path",
        },
        "reloaded": reloaded,
    })
}

/// The same report as lines.
pub(super) fn described_text(name: &str, host: &str, grant: &Value) -> Vec<String> {
    vec![
        format!("bucket          {name}"),
        format!("host            {host}"),
        format!("grant           {grant}"),
        format!(
            "secret key      vault item {} field {FIELD}",
            crate::config::namespace_item(name)
        ),
        format!("s3 path         {}/{name}", crate::remote::sigv4::S3_ROOT),
        format!("access key id   {name}"),
        format!("region          {}", crate::remote::sigv4::STADO_REGION),
    ]
}

/// `--json` prints the value as every storage command does; otherwise the
/// lines.
pub(super) fn answer(value: &Value, json: bool, text: &[String]) -> Result<(), CmdError> {
    if json {
        return crate::cli::storage::echo_json(value);
    }
    for line in text {
        println!("{line}");
    }
    Ok(())
}
