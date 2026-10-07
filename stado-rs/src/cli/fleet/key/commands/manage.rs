//! `key add|ls|rm|install|check` — the operator-facing commands over one
//! target's stored pair.

use crate::cli::CmdError;
use crate::deploy::{CommandSpec, Runner};
use crate::primitives::failure::FailureCode;
use serde_json::json;

use super::super::channel_argv;
use super::super::store::{
    authorized_keys_line, channel_destination, configured_client, item_id, run_checked,
    settle_readable, ITEM_PREFIX, ITEM_TYPE,
};

/// One key command's answer: the sentence a person reads, or with `--json`
/// the document carrying the same facts.
pub(in crate::cli::fleet::key) fn answer(
    as_json: bool,
    document: &serde_json::Value,
    sentence: &str,
) -> Result<bool, CmdError> {
    if as_json {
        crate::cli::print_answer(document, true)?;
    } else {
        println!("{sentence}");
    }
    Ok(true)
}

/// `key add TARGET --from PATH` — move an existing private key into the
/// selected store. The source file is removed only after a read-back verifies
/// the stored material; private content is never printed.
pub async fn add(runner: &Runner, target: &str, from: &str, as_json: bool) -> Result<bool, CmdError> {
    let metadata = std::fs::symlink_metadata(from)
        .map_err(|exc| CmdError::from(exc).within(format!("cannot inspect key file {from}")))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(CmdError::refused(format!(
            "key source {from} must be a regular file, not a symlink or special file"
        )));
    }
    let private_key = std::fs::read_to_string(from)
        .map_err(|exc| CmdError::from(exc).within(format!("cannot read key file {from}")))?;
    let public_key = run_checked(
        runner,
        CommandSpec::new(vec![
            "ssh-keygen".to_string(),
            "-y".to_string(),
            "-f".to_string(),
            from.to_string(),
        ]),
        "ssh-keygen -y",
        FailureCode::Refused,
    )
    .await?;
    let fingerprint_line = run_checked(
        runner,
        CommandSpec::new(vec![
            "ssh-keygen".to_string(),
            "-lf".to_string(),
            from.to_string(),
        ]),
        "ssh-keygen -lf",
        FailureCode::Refused,
    )
    .await?;
    let fingerprint = fingerprint_line
        .split_whitespace()
        .find(|part| part.starts_with("SHA256:"))
        .unwrap_or_default()
        .to_string();
    let key_type = fingerprint_line
        .rsplit('(')
        .next()
        .map(|part| part.trim().trim_end_matches(')').to_string())
        .unwrap_or_default();
    let id = item_id(target);
    let client = configured_client()?;
    client
        .write_described(
            &id,
            ITEM_TYPE,
            &json!({
                "private_key": private_key.trim(),
                "public_key": public_key.trim(),
            }),
            &json!({
                "key_type": key_type,
                "fingerprint": fingerprint,
                "added_at": chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await?;
    // The source file is about to be deleted, so the read-back is the only
    // thing standing between a half-written key and a key that exists nowhere.
    // It reads the material by name through the consumer the channel uses:
    // `fingerprint` is schema context rather than a field, carries no grant,
    // and proves nothing about whether this key can open a connection.
    if let Err(error) = settle_readable(
        &client,
        &id,
        &[
            ("private_key", private_key.trim()),
            ("public_key", public_key.trim()),
        ],
    )
    .await
    {
        let _ = client.delete_item(&id).await;
        return Err(error.within(format!(
            "credential item {id} failed read-back verification; the source file was preserved"
        )));
    }
    if let Err(error) = std::fs::remove_file(from) {
        let rollback = client.delete_item(&id).await;
        let failure = CmdError::from(error).within(format!("cannot remove source key {from}"));
        return Err(match rollback {
            Ok(()) => failure.also("the credential-store write was rolled back"),
            Err(rollback_error) => {
                failure.also(format!("store rollback also failed: {rollback_error}"))
            }
        });
    }
    let _ = std::fs::remove_file(format!("{from}.pub"));
    answer(
        as_json,
        &json!({ "target": target, "item": id, "fingerprint": fingerprint }),
        &format!("moved key into credential item {id} ({fingerprint})"),
    )
}

/// `key ls [--json]` — metadata of every stored SSH host key. No private fields.
pub async fn ls(json_output: bool) -> Result<bool, CmdError> {
    let client = configured_client()?;
    let items = client.list_items().await?;
    let mut shown = Vec::new();
    for item in items {
        if !item.id.starts_with(ITEM_PREFIX) {
            continue;
        }
        // `fingerprint` and `key_type` are schema CONTEXT on a `key-pair`, not
        // fields: Skarbiec's canonical form keeps the two halves of the key as
        // fields and everything descriptive beside them. The private field is
        // never asked for. A context that cannot be read is that item's error,
        // not two blank columns that read as a key with no fingerprint.
        let context = client
            .read_field(&item.id, "context")
            .await
            .map_err(|exc| {
                CmdError::from(exc).within(format!(
                    "cannot read the context of credential item {}",
                    item.id
                ))
            })?;
        let described = |name: &str| {
            context
                .get(name)
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default()
        };
        shown.push(json!({
            "item": item.id,
            "key_type": described("key_type"),
            "fingerprint": described("fingerprint"),
        }));
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&shown)?);
    } else if shown.is_empty() {
        println!("no SSH host keys in the credential store");
    } else {
        for row in &shown {
            let text = |name: &str| row[name].as_str().unwrap_or_default().to_string();
            println!(
                "{}\t{}\t{}",
                text("item"),
                text("key_type"),
                text("fingerprint")
            );
        }
    }
    Ok(true)
}

/// `key rm TARGET` — delete the target's SSH host key.
pub async fn rm(target: &str, as_json: bool) -> Result<bool, CmdError> {
    let client = configured_client()?;
    client.delete_item(&item_id(target)).await?;
    if as_json {
        let answer = serde_json::json!({ "target": target, "removed": item_id(target) });
        crate::cli::print_answer(&answer, true)?;
    } else {
        println!("removed credential item {}", item_id(target));
    }
    Ok(true)
}

/// `key install TARGET` — append the stored public key to the target's
/// authorized_keys through the existing credential-store-backed channel.
pub async fn install(runner: &Runner, target: &str, as_json: bool) -> Result<bool, CmdError> {
    let client = configured_client()?;
    let public_key = client
        .read_declared_string(&item_id(target), "public_key")
        .await?
        .ok_or_else(|| {
            CmdError::missing(format!(
                "credential item {} has no public_key field",
                item_id(target)
            ))
        })?;
    let destination = channel_destination(runner, target).await?;
    let destination = destination.as_str();
    let line = authorized_keys_line(&public_key, &item_id(target));
    let command = format!(
        "mkdir -p \"$HOME/.ssh\" && touch \"$HOME/.ssh/authorized_keys\" && grep -qF '{line}' \"$HOME/.ssh/authorized_keys\" || echo '{line}' >> \"$HOME/.ssh/authorized_keys\""
    );
    let (argv, _key) = channel_argv(target, destination, &command).await?;
    run_checked(
        runner,
        CommandSpec::new(argv),
        "authorized_keys install",
        FailureCode::InfraDown,
    )
    .await?;
    answer(
        as_json,
        &json!({ "target": target, "installed": true, "destination": destination }),
        &format!("installed public key for '{target}' into authorized_keys on {destination}"),
    )
}

/// `key check TARGET` — verify the selected-store key opens the channel.
pub async fn check(runner: &Runner, target: &str, as_json: bool) -> Result<bool, CmdError> {
    let destination = channel_destination(runner, target).await?;
    let destination = destination.as_str();
    let (argv, _key) = channel_argv(target, destination, "hostname").await?;
    let answered = run_checked(
        runner,
        CommandSpec::new(argv),
        "hostname over the channel",
        FailureCode::InfraDown,
    )
    .await?;
    answer(
        as_json,
        &json!({ "target": target, "destination": destination, "answered_as": answered.trim() }),
        &format!(
            "credential-store key verified: {destination} answered as {}",
            answered.trim()
        ),
    )
}
