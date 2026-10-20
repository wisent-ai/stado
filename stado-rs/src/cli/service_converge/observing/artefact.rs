//! Where an installed product lives, what version it carries, and whether
//! those bytes are the copy delivery staged for that version, from which
//! revision.

use serde_json::Value;

use crate::deploy::{host_channel, host_release, DeployError, Runner};
use crate::targets::ComputeTarget;

use crate::cli::service_converge::model::vocabulary::{
    ATTEST_ABSENT, ATTEST_DIFFERS, ATTEST_MATCH, ATTEST_NEVER_DELIVERED, ATTEST_UNKNOWN,
};

/// The SLSA v1 provenance field that names the tag a build was made from,
/// as a JSON Pointer (RFC 6901): one field path of the provenance schema.
const SLSA_TAG_POINTER: &str = "/buildDefinition/externalParameters/tag";

/// Where an installed product lives, or nothing. Candidates and never a
/// search: a probe that walks the filesystem looking for something called
/// <name> finds a backup copy and reports its version as the running one.
pub(super) async fn artefact_root(
    target: &ComputeTarget,
    runner: &Runner,
    home: &str,
    name: &str,
) -> Result<String, DeployError> {
    let stem = match name.split_once('-') {
        Some((stem, _)) => stem,
        None => name,
    };
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

/// The version a `.weles-release` stamp names: `version=` when the stamp
/// carries one, otherwise the version segment of the immutable coordinate
/// the artefact was fetched from:
///   release_uri=stado://releases/<product>/<version>/<platform>/<archive>
fn stamped_version(stamp: &str) -> Option<&str> {
    for line in stamp.lines() {
        if let Some(value) = line.strip_prefix("version=") {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    for line in stamp.lines() {
        let Some(rest) = line.strip_prefix("release_uri=stado://releases/") else {
            continue;
        };
        let mut segments = rest.split('/');
        if let (Some(_product), Some(version), Some(_platform)) =
            (segments.next(), segments.next(), segments.next())
        {
            return Some(version);
        }
    }
    None
}

/// The version an installed release artefact carries about itself:
/// `package.json` first — the version source a released product declares for
/// itself in `.wisent-release.json` — then the `.weles-release` stamp the
/// release launcher writes beside the unpacked runtime, then the SLSA
/// `provenance.json` shipped inside the artefact: its `version`, else the
/// tag its build definition names.
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
        if let Some(version) = stamped_version(&stamp).and_then(host_channel::extract_semver) {
            return Ok(version);
        }
    }
    let provenance = format!("{root}/provenance.json");
    let tag: Vec<&str> = SLSA_TAG_POINTER
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    for keys in [&["version"][..], &tag[..]] {
        if let Some(text) =
            host_channel::remote_json_member(target, &provenance, keys, runner).await?
        {
            if let Some(version) = host_channel::extract_semver(&text) {
                return Ok(version);
            }
        }
    }
    Ok(String::new())
}

/// What the host's delivery records say about the installed artefact: the
/// attestation word, the receipt's summary for the table, and the revision
/// the receipt says the delivered release was built from.
pub(super) struct Attested {
    pub(super) attestation: &'static str,
    pub(super) receipt: String,
    /// `source_commit` of the receipt beside the staged copy; `None` when
    /// the bytes are not attested, the delivery predates receipts, or the
    /// receipt names no revision.
    pub(super) revision: Option<String>,
}

impl Attested {
    fn bare(attestation: &'static str) -> Self {
        Self {
            attestation,
            receipt: String::new(),
            revision: None,
        }
    }
}

/// Whether the installed artefact is the copy the delivery path staged for
/// the version it claims, and which revision that delivery came from.
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
) -> Result<Attested, DeployError> {
    if version.is_empty() || root.is_empty() || !host_release::is_exact_semver(version) {
        return Ok(Attested::bare(ATTEST_UNKNOWN));
    }
    let platform = target.release_platform.trim();
    if platform.is_empty() {
        return Ok(Attested::bare(ATTEST_UNKNOWN));
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
            return Ok(Attested::bare(ATTEST_ABSENT));
        }
        return Ok(Attested::bare(ATTEST_NEVER_DELIVERED));
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
    .succeeded();
    if !same {
        return Ok(Attested::bare(ATTEST_DIFFERS));
    }
    // The receipt release delivery leaves beside the staged copy, when there is
    // one. A delivery made before that format has none, and its absence is
    // "installed before receipts" rather than a finding: the byte comparison
    // above has already attested these bytes without it. A receipt that is
    // there but cannot be read or parsed is said with its cause.
    let receipt_path = format!("{coordinate}/release-receipt.json");
    let quoted_receipt = crate::deploy::shlex_quote(&receipt_path);
    if !host_channel::remote_test(target, &format!("-f {quoted_receipt}"), runner).await? {
        return Ok(Attested::bare(ATTEST_MATCH));
    }
    let read =
        host_channel::run_command(target, &format!("/bin/cat {quoted_receipt}"), runner).await?;
    if !read.succeeded() {
        return Ok(Attested {
            attestation: ATTEST_MATCH,
            receipt: format!(
                "its release receipt {receipt_path} could not be read: {}",
                host_channel::whole_failure(&read)
            ),
            revision: None,
        });
    }
    match serde_json::from_str::<Value>(read.stdout.trim()) {
        Ok(document) => Ok(Attested {
            attestation: ATTEST_MATCH,
            receipt: format!(
                "delivered {} by {}",
                crate::cli::reporting::field(&document["installed_at"]),
                crate::cli::reporting::field(&document["delivered_by"])
            ),
            revision: match document["source_commit"].as_str() {
                Some(commit) if !commit.is_empty() => Some(commit.to_string()),
                _ => None,
            },
        }),
        Err(error) => Ok(Attested {
            attestation: ATTEST_MATCH,
            receipt: format!("its release receipt {receipt_path} is not JSON: {error}"),
            revision: None,
        }),
    }
}
