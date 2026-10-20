//! The reporter: one remote command per question, composed into the report
//! text this module's parse reads back.

use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

use crate::cli::service_converge::model::vocabulary::{NONE, UNKNOWN};
use crate::cli::service_converge::observing::artefact::{
    artefact_root, artefact_version, attest_installed,
};
use crate::cli::service_converge::observing::units::{
    declared_service_records, unit_for_root, unit_state,
};

/// Report, for every binary this host has a declared `managed_versions` entry
/// for, which version it is actually running — natively, one remote command
/// per question, with the report text composed here in the wire format
/// `parse_report` reads.
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
/// `release version show` reports it as `unknown`, never as `in-sync`. Beside
/// it, `revision=` names the revision the delivery receipt says the staged
/// copy was built from, or `none`.
///
/// Read-only, and strictly so: nothing is fetched, nothing is written, no
/// unit is restarted, and no credential is printed — the only values emitted
/// are binary names, versions, revisions, paths, unit labels and launchd
/// state.
pub(super) async fn probe_installed_versions(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<String, DeployError> {
    let home = host_channel::remote_home(target, runner).await?;
    let services = declared_service_records(target);
    let mut uid = None;
    let mut out = format!(
        "# host {}  registry canonical  at {}\n",
        target.name,
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    );
    for binary in target.managed_versions.keys() {
        let stado_program = format!("{home}/.stado/bin/{binary}");
        let quoted_program = crate::deploy::shlex_quote(&stado_program);
        let (root, version) =
            if host_channel::remote_test(target, &format!("-x {quoted_program}"), runner).await?
                && host_channel::remote_test(target, &format!("-f {quoted_program}"), runner)
                    .await?
            {
                // A program that names no version reports `unknown` below.
                let version =
                    match host_channel::remote_program_version(target, &stado_program, runner)
                        .await?
                    {
                        Some(version) => version,
                        None => String::new(),
                    };
                (stado_program, version)
            } else {
                let root = artefact_root(target, runner, &home, binary).await?;
                let version = if root.is_empty() {
                    String::new()
                } else {
                    artefact_version(target, runner, &root).await?
                };
                (root, version)
            };

        // Provenance, asked host-locally: the delivery path stages every
        // release it installs at
        // `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`,
        // digest-verified against the canonical manifest on the way in. If the
        // version these bytes claim has no staged copy, or the installed file
        // is not that copy, the bytes did not come through delivery.
        let attested = attest_installed(target, runner, &home, binary, &root, &version).await?;

        let mut unit = NONE.to_string();
        let mut state = NONE.to_string();
        if let Some((label, path, kind)) =
            unit_for_root(target, runner, &home, &services, &root).await?
        {
            state = unit_state(target, runner, &label, &path, &kind, &mut uid).await?;
            unit = label;
        }

        out.push_str(&format!(
            "binary={} version={} root={} unit={} state={} attestation={} revision={} receipt={}\n",
            binary,
            if version.is_empty() {
                UNKNOWN
            } else {
                &version
            },
            if root.is_empty() { NONE } else { &root },
            unit,
            state,
            attested.attestation,
            match &attested.revision {
                Some(revision) => revision.as_str(),
                None => NONE,
            },
            // One token, so the line stays `key=value` and a receipt with a
            // space in it cannot become two fields.
            if attested.receipt.is_empty() {
                NONE.to_string()
            } else {
                attested.receipt.replace(' ', "_")
            },
        ));
    }
    out.push_str(&format!("# binaries {}\n", target.managed_versions.len()));
    Ok(out)
}
