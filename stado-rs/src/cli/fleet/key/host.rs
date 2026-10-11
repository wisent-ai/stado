//! What a key rotation does on the target host: install the new public key
//! beside the old one, prove the channel opens with it, and take the old one
//! off.

use crate::cli::CmdError;
use crate::deploy::{CommandSpec, Runner};
use crate::primitives::failure::FailureCode;

use super::{authorized_keys_line, channel_argv, channel_destination, item_id, run_checked};

/// Add `public_key` to `target`'s `authorized_keys` through the channel,
/// once.
pub(super) async fn install_public_key(
    runner: &Runner,
    target: &str,
    public_key: &str,
) -> Result<(), CmdError> {
    let destination = channel_destination(runner, target).await?;
    let line = authorized_keys_line(public_key, &item_id(target));
    let command = format!(
        "mkdir -p \"$HOME/.ssh\" && touch \"$HOME/.ssh/authorized_keys\" && grep -qF '{line}' \"$HOME/.ssh/authorized_keys\" || echo '{line}' >> \"$HOME/.ssh/authorized_keys\""
    );
    let (argv, _key) = channel_argv(target, &destination, &command).await?;
    run_checked(
        runner,
        CommandSpec::new(argv),
        "authorized_keys install",
        FailureCode::InfraDown,
    )
    .await
    .map(|_| ())
}

/// Take `public_key` off `target`'s `authorized_keys`; an empty key names
/// nothing to take off.
pub(super) async fn remove_public_key(
    runner: &Runner,
    target: &str,
    public_key: &str,
) -> Result<(), CmdError> {
    if public_key.is_empty() {
        return Ok(());
    }
    let destination = channel_destination(runner, target).await?;
    let command = format!(
        "grep -vF '{public_key}' \"$HOME/.ssh/authorized_keys\" > \"$HOME/.ssh/authorized_keys.tmp\" && mv \"$HOME/.ssh/authorized_keys.tmp\" \"$HOME/.ssh/authorized_keys\""
    );
    let (argv, _key) = channel_argv(target, &destination, &command).await?;
    run_checked(
        runner,
        CommandSpec::new(argv),
        "authorized_keys cleanup",
        FailureCode::InfraDown,
    )
    .await
    .map(|_| ())
}

/// Open the channel to `target` with the key the store now holds, and answer
/// the hostname it reached.
pub(super) async fn verify_new_key(runner: &Runner, target: &str) -> Result<String, CmdError> {
    let destination = channel_destination(runner, target).await?;
    let (argv, _key) = channel_argv(target, &destination, "hostname").await?;
    Ok(run_checked(
        runner,
        CommandSpec::new(argv),
        "hostname with the new key",
        FailureCode::InfraDown,
    )
    .await?
    .trim()
    .to_string())
}
