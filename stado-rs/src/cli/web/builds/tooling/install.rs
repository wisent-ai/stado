//! The locked install, and how it fetches a private Git dependency without
//! leaving a credential on the host: from the release input that answers it
//! (`--git-input`), else over HTTPS with a declared `GITHUB_TOKEN`.

use std::path::Path;

use crate::cli::web::builds::tooling::inputs::{origins, redirected};
use crate::cli::web::builds::tooling::node::npm;
use crate::cli::CmdError;

/// Every private GitHub dependency the lockfile pins, as npm will hand it to
/// Git: its SSH URL, up to the revision it pins.
///
/// A private Git dependency is the one input a fleet host cannot fetch by
/// being a fleet host: `npm ci` shells out to `git ls-remote` on that SSH
/// URL, which needs a GitHub SSH key in that account. On Vercel the same
/// products installed over HTTPS, because their `installCommand` rewrote the
/// URL with `GITHUB_TOKEN` first; without an answer here a build runs for
/// half an hour and then fails with Git's `Permission denied (publickey)`.
///
/// A lockfile that cannot be read is refused: read as pinning none, the
/// answer was skipped and the build failed half an hour later on the SSH key.
fn ssh_github_dependencies(lockfile: &Path) -> Result<Vec<String>, CmdError> {
    let body = std::fs::read_to_string(lockfile).map_err(|error| {
        CmdError::click(format!("cannot read {}: {error}", lockfile.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let prefix = origins::ssh();
    let mut pinned: Vec<String> = body
        .match_indices(prefix.as_str())
        .map(|(start, _)| {
            body[start..]
                .chars()
                .take_while(|character| *character != '#' && *character != '"')
                .collect()
        })
        .collect();
    pinned.sort();
    pinned.dedup();
    Ok(pinned)
}

/// The rewrite that lets a declared `GITHUB_TOKEN` fetch what no release
/// input answers, as Git's own environment-scoped configuration; nothing when
/// no token is declared.
///
/// `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n` apply to every
/// git process this install spawns and to nothing else: no file is written, so
/// a build cannot leave a credential in `~/.gitconfig` on a shared host, and a
/// second product building beside it is unaffected. The token is already in
/// this process's environment because the product's manifest declares it under
/// `secret_env`, which is also what makes a host that cannot read it decline
/// the job instead of failing it. A release input's rewrite names a whole
/// repository, which is longer than this one's prefix, so Git still answers
/// that repository from its input.
fn github_git_rewrite(variables: &[(String, String)]) -> Result<Vec<(String, String)>, CmdError> {
    // `secret_env` is delivered into this process's environment by the release
    // worker and never appears in the manifest's `env`, which is the only
    // thing [`declared_env`] reads. Both are consulted, environment last, so a
    // product that declares a public token in `env` and one that declares a
    // vault reference in `secret_env` build the same way.
    let declared = variables
        .iter()
        .find(|(name, _)| name == "GITHUB_TOKEN")
        .map(|(_, value)| value.trim().to_string());
    let token = match declared {
        Some(token) => token,
        None => match std::env::var("GITHUB_TOKEN") {
            Ok(value) => value.trim().to_string(),
            Err(std::env::VarError::NotPresent) => return Ok(Vec::new()),
            Err(error) => {
                let refusal = CmdError::click(format!("GITHUB_TOKEN cannot be read: {error}"));
                return Err(refusal.stating(crate::primitives::failure::FailureCode::Config));
            }
        },
    };
    if token.is_empty() {
        return Ok(Vec::new());
    }
    let https = format!("url.{}.insteadOf", origins::https_with(&token));
    Ok(vec![
        ("GIT_CONFIG_COUNT".to_string(), "2".to_string()),
        ("GIT_CONFIG_KEY_0".to_string(), https.clone()),
        ("GIT_CONFIG_VALUE_0".to_string(), origins::ssh()),
        ("GIT_CONFIG_KEY_1".to_string(), https),
        ("GIT_CONFIG_VALUE_1".to_string(), origins::scp()),
    ])
}

/// Install exactly the tree the lockfile pins.
///
/// `npm install` is deliberately not allowed in its place. It resolves versions afresh,
/// so the artifact would carry whatever was newest on the registry the minute
/// the builder ran, and two builds of one commit would ship different
/// dependencies. An unlocked install is not a release input.
///
/// Every private GitHub dependency the lockfile pins is answered by the
/// release input the manifest declares for it, or fetched with a declared
/// `GITHUB_TOKEN`; one neither answers is refused by name before `npm ci`
/// runs, so the build never waits half an hour on an SSH key.
pub(in crate::cli::web::builds) fn install(
    source: &Path,
    variables: &[(String, String)],
) -> Result<(), CmdError> {
    let lockfile = source.join("package-lock.json");
    if !lockfile.is_file() {
        return Err(CmdError::click(format!(
            "{} is missing: a web release installs only what a lockfile pins, and Stado will not use `npm install` instead because that resolves versions at build time and makes the artifact unreproducible",
            lockfile.display()
        )).stating(crate::primitives::failure::FailureCode::Config));
    }
    let mut variables = variables.to_vec();
    let pinned = ssh_github_dependencies(&lockfile)?;
    if !pinned.is_empty() {
        let answered = redirected(source, &variables)?;
        let unanswered: Vec<&str> = pinned
            .iter()
            .map(String::as_str)
            .filter(|url| {
                !answered
                    .iter()
                    .any(|origin| url.starts_with(origin.as_str()))
            })
            .collect();
        if unanswered.is_empty() {
            println!(
                "stado web build: {} pins private git dependencies; the declared release inputs answer every one, so none is fetched from GitHub",
                lockfile.display()
            );
        } else {
            let rewrite = github_git_rewrite(&variables)?;
            if rewrite.is_empty() {
                return Err(CmdError::click(format!(
                    "{} pins private git dependencies that no release input answers and no \
                     GITHUB_TOKEN is declared to fetch: {}. `npm ci` would ask git for an SSH key \
                     this host does not have. Declare a Git bundle input for each in the product's \
                     .wisent-release.json (--git-input INPUT=OWNER/REPOSITORY.git), or declare the \
                     token under platforms.<platform>.secret_env, for example \
                     \"secret_env\": {{\"GITHUB_TOKEN\": \"GITHUB_TOKEN#value\"}}",
                    lockfile.display(),
                    unanswered.join(", ")
                ))
                .stating(crate::primitives::failure::FailureCode::Config));
            }
            println!(
                "stado web build: {} pins private git dependencies no release input answers ({}); \
                 fetching them over HTTPS with the declared GITHUB_TOKEN, for this install only",
                lockfile.display(),
                unanswered.join(", ")
            );
            variables.extend(rewrite);
        }
    }
    // A postinstall script is part of the build, so the declared variables
    // reach it too: a product whose install step reads its public origin must
    // not see it only later.
    npm(source, &["ci", "--no-audit", "--no-fund"], &variables)
}
