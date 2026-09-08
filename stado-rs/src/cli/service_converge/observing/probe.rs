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
    let mut count = 0usize;
    for binary in target.managed_versions.keys() {
        count += 1;

        let stado_program = format!("{home}/.stado/bin/{binary}");
        let quoted_program = crate::deploy::shlex_quote(&stado_program);
        let (root, version) =
            if host_channel::remote_test(target, &format!("-x {quoted_program}"), runner).await?
                && host_channel::remote_test(target, &format!("-f {quoted_program}"), runner)
                    .await?
            {
                let version = host_channel::remote_program_version(target, &stado_program, runner)
                    .await?
                    .unwrap_or_default();
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
        let (attestation, receipt) =
            attest_installed(target, runner, &home, binary, &root, &version).await?;

        let mut unit = "none".to_string();
        let mut state = "none".to_string();
        if let Some((label, path, kind)) =
            unit_for_root(target, runner, &home, &services, &root).await?
        {
            state = unit_state(target, runner, &label, &path, &kind, &mut uid).await?;
            unit = label;
        }

        out.push_str(&format!(
            "binary={} version={} root={} unit={} state={} attestation={} receipt={}\n",
            binary,
            if version.is_empty() {
                UNKNOWN
            } else {
                &version
            },
            if root.is_empty() { "none" } else { &root },
            unit,
            state,
            attestation,
            // One token, so the line stays `key=value` and a receipt with a
            // space in it cannot become two fields.
            if receipt.is_empty() {
                NONE.to_string()
            } else {
                receipt.replace(' ', "_")
            },
        ));
    }
    out.push_str(&format!("# binaries {count}\n"));
    Ok(out)
}
