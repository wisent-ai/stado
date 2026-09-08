//! Where an installed product lives, what version it carries, and whether
//! those bytes are the copy delivery staged for that version.

use serde_json::Value;

use crate::deploy::{host_channel, host_release, DeployError, Runner};
use crate::targets::ComputeTarget;

use crate::cli::service_converge::model::vocabulary::{
    ATTEST_ABSENT, ATTEST_DIFFERS, ATTEST_MATCH, ATTEST_NEVER_DELIVERED, ATTEST_UNKNOWN,
};

/// Where an installed product lives, or nothing. Candidates and never a
/// search: a probe that walks the filesystem looking for something called
/// <name> finds a backup copy and reports its version as the running one.
pub(super) async fn artefact_root(
    target: &ComputeTarget,
    runner: &Runner,
    home: &str,
    name: &str,
) -> Result<String, DeployError> {
    let stem = name.split('-').next().unwrap_or(name);
    for candidate in [
        format!("{home}/{name}"),
        format!("{home}/{stem}"),
        format!("{home}/.stado/releases/{name}/current"),
        format!("/opt/{name}"),
    ] {
        if host_channel::remote_test(
            target,
            &format!("-d {}", crate::deploy::shlex_quote(&candidate)),
            runner,
        )
        .await?
        {
            return Ok(candidate);
        }
    }
    Ok(String::new())
}

/// The version an installed release artefact carries about itself:
/// `package.json` first — the version source a released product declares for
/// itself in `.wisent-release.json` — then the `.weles-release` stamp the
/// release launcher writes beside the unpacked runtime, then the SLSA
/// `provenance.json` shipped inside the artefact.
pub(super) async fn artefact_version(
    target: &ComputeTarget,
    runner: &Runner,
    root: &str,
) -> Result<String, DeployError> {
    if let Some(text) = host_channel::remote_json_member(
        target,
        &format!("{root}/package.json"),
        &["version"],
        runner,
    )
    .await?
    {
        if let Some(version) = host_channel::extract_semver(&text) {
            return Ok(version);
        }
    }
    if let Some(stamp) =
        host_channel::remote_read_file(target, &format!("{root}/.weles-release"), runner).await?
    {
        // `version=` when the stamp carries one, otherwise the version
        // segment of the immutable coordinate the artefact was fetched from:
        //   release_uri=stado://releases/<product>/<version>/<platform>/<archive>
        let stamped = stamp
            .lines()
            .find_map(|line| line.strip_prefix("version="))
            .filter(|value| !value.is_empty())
            .or_else(|| {
                stamp.lines().find_map(|line| {
                    let rest = line.strip_prefix("release_uri=stado://releases/")?;
                    let mut segments = rest.split('/');
                    segments.next()?;
                    let version = segments.next()?;
                    segments.next().map(|_| version)
                })
            });
        if let Some(version) = stamped.and_then(host_channel::extract_semver) {
            return Ok(version);
        }
    }
    for keys in [
        &["version"][..],
        &["buildDefinition", "externalParameters", "tag"][..],
    ] {
        if let Some(text) = host_channel::remote_json_member(
            target,
            &format!("{root}/provenance.json"),
            keys,
            runner,
        )
        .await?
        {
            if let Some(version) = host_channel::extract_semver(&text) {
                return Ok(version);
            }
        }
    }
    Ok(String::new())
}

/// Report, for every binary this host has a declared `managed_versions` entry
/// for, which version it is actually running — natively, one remote command
/// per question, with the report text composed here in the wire format
/// [`parse_report`] reads.
///
/// The declaration is the scope — a binary nobody declared is not this
/// reporter's business, and reporting it would bury the ones that are. Where
/// an installed version comes from, in this order, first hit wins:
///
///   1. `$HOME/.stado/bin/<name>` — an owner-only Stado program, asked
///      directly;
///   2. package.json `version`;
///   3. `.weles-release`;
///   4. `provenance.json` — `.version` when it carries one, otherwise the
///      build's own tag.
///
/// A product whose artefact carries none of those reports `version=unknown`.
/// That is the honest answer and it is never rounded to the declared version:
/// `service converge` reports it as `unknown`, never as `in-sync`.
///
/// Read-only, and strictly so: nothing is fetched, nothing is written, no
/// unit is restarted, and no credential is printed — the only values emitted
/// are binary names, versions, paths, unit labels and launchd state.
/// Whether the installed artefact is the copy the delivery path staged for
/// the version it claims.
///
/// Host-local and cheap: `cmp -s` against
/// `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, which
/// release delivery writes and verifies against the canonical manifest's
/// SHA-256 before it installs anything. No network, no manifest fetch, and no
/// second opinion needed about what a version string means — a local build
/// claiming a released version has no staged copy to match.
pub(super) async fn attest_installed(
    target: &ComputeTarget,
    runner: &Runner,
    home: &str,
    binary: &str,
    root: &str,
    version: &str,
) -> Result<(&'static str, String), DeployError> {
    if version.is_empty() || root.is_empty() || !host_release::is_exact_semver(version) {
        return Ok((ATTEST_UNKNOWN, String::new()));
    }
    let platform = target.release_platform.trim();
    if platform.is_empty() {
        return Ok((ATTEST_UNKNOWN, String::new()));
    }
    let coordinate = format!("{home}/.stado/releases/{binary}/{version}/{platform}");
    let staged = format!("{coordinate}/{binary}");
    let quoted_staged = crate::deploy::shlex_quote(&staged);
    if !host_channel::remote_test(target, &format!("-f {quoted_staged}"), runner).await? {
        // Release delivery creates `<binary>/<version>/<platform>/` only when it
        // stages, so the binary directory existing at all is the record that
        // this host has been delivered to before. Its absence is bootstrap,
        // not tampering.
        let history = format!("{home}/.stado/releases/{binary}");
        let quoted_history = crate::deploy::shlex_quote(&history);
        if host_channel::remote_test(target, &format!("-d {quoted_history}"), runner).await? {
            return Ok((ATTEST_ABSENT, String::new()));
        }
        return Ok((ATTEST_NEVER_DELIVERED, String::new()));
    }
    // A byte comparison, not a digest: the two files are already on the same
    // disk, `cmp -s` reads no further than the first difference, and there is
    // no hash to agree on between this process and the host.
    let same = host_channel::run_command(
        target,
        &format!(
            "/usr/bin/cmp -s {} {quoted_staged}",
            crate::deploy::shlex_quote(root)
        ),
        runner,
    )
    .await?
    .ok();
    if !same {
        return Ok((ATTEST_DIFFERS, String::new()));
    }
    // The receipt release delivery leaves beside the staged copy, when there is
    // one. A delivery made before that format has none, and its absence is
    // "installed before receipts" rather than a finding: the byte comparison
    // above has already attested these bytes without it.
    let receipt = host_channel::run_command(
        target,
        &format!(
            "/bin/cat {}/release-receipt.json 2>/dev/null || true",
            crate::deploy::shlex_quote(&coordinate)
        ),
        runner,
    )
    .await?;
    let summary = serde_json::from_str::<Value>(receipt.stdout.trim())
        .ok()
        .map(|document| {
            let field = |key: &str| {
                document
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            (field("installed_at"), field("delivered_by"))
        })
        .filter(|(at, by)| !at.is_empty() || !by.is_empty())
        .map(|(at, by)| format!("delivered {at} by {by}"))
        .unwrap_or_default();
    Ok((ATTEST_MATCH, summary))
}
