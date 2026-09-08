//! The locked install, and the one rewrite that lets it fetch a private git
//! dependency without leaving a credential on the host.

use std::path::Path;

use crate::cli::web::builds::tooling::node::npm;
use crate::cli::CmdError;

/// Every `ssh://git@github.com/` dependency the lockfile pins, as npm will
/// hand them to git.
///
/// A private git dependency is the one input a fleet host cannot fetch by
/// being a fleet host: `npm ci` shells out to `git ls-remote ssh://git@…`,
/// which needs a GitHub SSH key in that account. On Vercel the same products
/// installed over HTTPS, because their `installCommand` rewrote the URL with
/// `GITHUB_TOKEN` first. On 2026-09-06 `preferences` 0.1.2 built on
/// charless-mac-mini for thirty-eight minutes and then failed with
/// `git@github.com: Permission denied (publickey)` on
/// `wisent-ai/deep-analytics`, because nothing did that rewrite here.
fn ssh_github_dependencies(lockfile: &Path) -> bool {
    std::fs::read_to_string(lockfile)
        .map(|body| body.contains("ssh://git@github.com/"))
        .unwrap_or_default()
}

/// The rewrite that lets a declared `GITHUB_TOKEN` fetch them, as git's own
/// environment-scoped configuration.
///
/// `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n` apply to every
/// git process this install spawns and to nothing else: no file is written, so
/// a build cannot leave a credential in `~/.gitconfig` on a shared host, and a
/// second product building beside it is unaffected. The token is already in
/// this process's environment because the product's manifest declares it under
/// `secret_env`, which is also what makes a host that cannot read it decline
/// the job instead of failing it.
fn github_git_rewrite(variables: &[(String, String)]) -> Vec<(String, String)> {
    // `secret_env` is delivered into this process's environment by the release
    // worker and never appears in the manifest's `env`, which is the only
    // thing [`declared_env`] reads. Both are consulted, environment last, so a
    // product that declares a public token in `env` and one that declares a
    // vault reference in `secret_env` build the same way.
    let token = variables
        .iter()
        .find(|(name, _)| name == "GITHUB_TOKEN")
        .map(|(_, value)| value.trim().to_string())
        .or_else(|| {
            std::env::var("GITHUB_TOKEN")
                .ok()
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty());
    let Some(token) = token else {
        return Vec::new();
    };
    let https = format!("url.https://x-access-token:{token}@github.com/.insteadOf");
    vec![
        ("GIT_CONFIG_COUNT".to_string(), "2".to_string()),
        ("GIT_CONFIG_KEY_0".to_string(), https.clone()),
        (
            "GIT_CONFIG_VALUE_0".to_string(),
            "ssh://git@github.com/".to_string(),
        ),
        ("GIT_CONFIG_KEY_1".to_string(), https),
        (
            "GIT_CONFIG_VALUE_1".to_string(),
            "git@github.com:".to_string(),
        ),
    ]
}

/// Install exactly the tree the lockfile pins.
///
/// `npm install` is deliberately not allowed in its place. It resolves versions afresh,
/// so the artifact would carry whatever was newest on the registry the minute
/// the builder ran, and two builds of one commit would ship different
/// dependencies. An unlocked install is not a release input.
pub(in crate::cli::web::builds) fn install(
    source: &Path,
    variables: &[(String, String)],
) -> Result<(), CmdError> {
    let lockfile = source.join("package-lock.json");
    if !lockfile.is_file() {
        return Err(CmdError::click(format!(
            "{} is missing: a web release installs only what a lockfile pins, and Stado will not use `npm install` instead because that resolves versions at build time and makes the artifact unreproducible",
            lockfile.display()
        )));
    }
    let mut variables = variables.to_vec();
    if ssh_github_dependencies(&lockfile) {
        let rewrite = github_git_rewrite(&variables);
        if rewrite.is_empty() {
            return Err(CmdError::click(format!(
                "{} pins dependencies over ssh://git@github.com/, and no GITHUB_TOKEN is declared \
                 for this build: `npm ci` would ask git for an SSH key this host does not have. \
                 Declare it in the product's .wisent-release.json under \
                 platforms.<platform>.secret_env, for example \
                 \"secret_env\": {{\"GITHUB_TOKEN\": \"GITHUB_TOKEN#value\"}}",
                lockfile.display()
            )));
        }
        println!(
            "stado web build: {} pins private git dependencies; fetching them over HTTPS with the \
             declared GITHUB_TOKEN, for this install only",
            lockfile.display()
        );
        variables.extend(rewrite);
    }
    // A postinstall script is part of the build, so the declared variables
    // reach it too: a product whose install step reads its public origin must
    // not see it only later.
    npm(source, &["ci", "--no-audit", "--no-fund"], &variables)
}
