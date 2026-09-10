//! The one-line remote primitives a caller composes from constants instead
//! of shipping a shell program: a file test, a home directory, a file read, a
//! program's version, one member of a JSON document.

use serde_json::Value;

use super::run_program;
use crate::deploy::{shlex_quote, CommandOutput, DeployError, Runner};
use crate::targets::ComputeTarget;

pub async fn run_command(
    target: &ComputeTarget,
    command: &str,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    run_program(target, &["/bin/sh", "-c", command], runner).await
}

/// Answer one remote `test`(1) expression (`-f path`, `-d dir`, …).
///
/// The expression is caller-composed from constants and [`shlex_quote`]d
/// paths. A command the transport could not deliver is an error; a test that
/// did not hold is `false`, never an error.
pub async fn remote_test(
    target: &ComputeTarget,
    expression: &str,
    runner: &Runner,
) -> Result<bool, DeployError> {
    Ok(run_command(target, &format!("test {expression}"), runner)
        .await?
        .ok())
}

/// The remote login user's home directory.
///
/// Every fixed path this channel's callers probe is anchored at `$HOME`, and
/// `$HOME` expands only on the far side, so it is read once, here, and the
/// concrete paths are composed by the caller.
pub async fn remote_home(target: &ComputeTarget, runner: &Runner) -> Result<String, DeployError> {
    let output = run_command(target, "printf '%s' \"$HOME\"", runner).await?;
    let home = output.stdout.trim();
    if !output.ok() || home.is_empty() || !home.starts_with('/') {
        return Err(DeployError(format!(
            "{}: the remote shell did not state its home directory",
            target.name
        )));
    }
    Ok(home.to_string())
}

/// The text of one remote file, or `None` when it is not a readable regular
/// file — the `[ -f "$path" ] || return 0` guard every retired reporter
/// opened with, and the read itself one fixed `cat`.
pub async fn remote_read_file(
    target: &ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<Option<String>, DeployError> {
    if !remote_test(target, &format!("-f {}", shlex_quote(path)), runner).await? {
        return Ok(None);
    }
    let output = run_program(target, &["/bin/cat", path], runner).await?;
    Ok(output.ok().then_some(output.stdout))
}

/// A version, or nothing, out of whatever text a program printed, anchored on
/// the semantic-version shape the release pack accepts: a banner line
/// ("stado 0.6.0"), a bare version and a `v`-prefixed tag all read the same,
/// and a sentence with no version in it reads as nothing.
///
/// Transcribed from the `extract_version` helper the retired shell reporters
/// shared (`tr ' \t' '\n\n'`, then one anchored sed match per line), token
/// for token: split on blanks, keep the first whole token that is an exact
/// `X.Y.Z` with an optional `-prerelease` tail.
pub fn extract_semver(text: &str) -> Option<String> {
    text.split([' ', '\t', '\n', '\r'])
        .filter(|token| !token.is_empty())
        .find_map(|token| {
            let bare = token
                .strip_prefix(|head| matches!(head, 'v' | 'V'))
                .unwrap_or(token);
            semver_token(bare).then(|| bare.to_string())
        })
}

/// One whole token of the reporters' version shape: `[vV]?X.Y.Z[-prerelease]`,
/// the `v` already stripped by the caller.
fn semver_token(token: &str) -> bool {
    let (core, prerelease) = match token.split_once('-') {
        Some((core, tail)) => (core, Some(tail)),
        None => (token, None),
    };
    let number = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let mut parts = core.split('.');
    let core_ok = matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(major), Some(minor), Some(patch), None)
            if number(major) && number(minor) && number(patch)
    );
    core_ok
        && prerelease.is_none_or(|tail| {
            !tail.is_empty()
                && tail
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        })
}

/// A program, asked what it is: `--version` first, because that is what every
/// Rust binary in this fleet answers to; the `version` subcommand second,
/// because skarbiec prints a JSON object from one. stdin is closed so a
/// program that would prompt cannot hold the read open. A program that prints
/// nothing parseable answers nothing — never a fabricated version.
pub async fn remote_program_version(
    target: &ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<Option<String>, DeployError> {
    let output = run_program(target, &[path, "--version"], runner).await?;
    if let Some(version) = extract_semver(&output.stdout) {
        return Ok(Some(version));
    }
    let output = run_program(target, &[path, "version"], runner).await?;
    let text = if output.stdout.trim_start().starts_with('{') {
        serde_json::from_str::<Value>(&output.stdout)
            .ok()
            .and_then(|document| match document.get("version") {
                Some(Value::String(version)) => Some(version.clone()),
                Some(Value::Number(version)) => Some(version.to_string()),
                _ => None,
            })
            .unwrap_or_default()
    } else {
        output.stdout
    };
    Ok(extract_semver(&text))
}

/// One JSON member of a remote file, by a path of object keys, or nothing:
/// a missing file, an unparseable file and an absent member are one answer on
/// purpose, because each means this file did not state the version and the
/// caller falls through to the next source.
pub async fn remote_json_member(
    target: &ComputeTarget,
    path: &str,
    keys: &[&str],
    runner: &Runner,
) -> Result<Option<String>, DeployError> {
    let Some(text) = remote_read_file(target, path, runner).await? else {
        return Ok(None);
    };
    let Ok(mut document) = serde_json::from_str::<Value>(&text) else {
        return Ok(None);
    };
    for key in keys {
        let Some(object) = document.as_object() else {
            return Ok(None);
        };
        document = object.get(*key).cloned().unwrap_or(Value::Null);
    }
    Ok(match &document {
        Value::String(member) => Some(member.clone()),
        Value::Number(member) => Some(member.to_string()),
        _ => None,
    })
}
