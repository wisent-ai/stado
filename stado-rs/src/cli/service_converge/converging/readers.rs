//! The runtime half: the same catalog-verified bytes into every private
//! reader tree.

use serde_json::json;

use crate::deploy::{host_channel, host_release, Runner};
use crate::targets::ComputeTarget;

use crate::cli::service_converge::model::receipts::{AppliedPass, Released, COMPLETED, FAILED};

/// Finish the runtime half even when the installed Stado file was already
/// attested and at the declared version.
///
/// Root delivery retains the exact catalog-verified archive beside the staged
/// binary. Pass that archive and its independently resolved catalog digest to
/// the target CLI: global-path owners are recycled first, then the existing
/// service-update implementation installs the same bytes into every private
/// reader tree before restarting and proving its process image.
pub(in crate::cli::service_converge) async fn converge_native_readers(
    target: &ComputeTarget,
    declared: &[(String, String)],
    runner: &Runner,
    pass: &mut AppliedPass,
) {
    let Some(version) = declared
        .iter()
        .find(|(name, _)| name == "stado")
        .map(|(_, version)| version.clone())
    else {
        return;
    };
    let (reader_target, request, self_store) = match host_release::resolve_release_request(
        &target.name,
        "stado",
        &version,
        false,
        false,
        runner,
    )
    .await
    {
        Ok(resolved) => resolved,
        Err(error) => {
            pass.releases.push(Released {
                binary: "stado-readers".to_string(),
                version,
                status: FAILED,
                detail: format!("cannot resolve the Stado reader archive: {error}"),
            });
            return;
        }
    };
    if let Err(error) =
        host_release::ensure_stado_reader_archive(&reader_target, &request, self_store, runner)
            .await
    {
        pass.releases.push(Released {
            binary: "stado-readers".to_string(),
            version,
            status: FAILED,
            detail: error.to_string(),
        });
        return;
    }
    let script = format!(
        "set -euo pipefail\n\
         archive=\"$HOME/.stado/releases/stado/{}/{}/{}\"\n\
         \"$HOME/.stado/bin/stado\" release converge-local-readers \
         --name stado --archive \"$archive\" --sha256 {}\n",
        request.version,
        request.platform,
        host_release::READER_ARCHIVE_NAME,
        crate::deploy::shlex_quote(&request.sha256),
    );
    let outcome = host_channel::run_script(&reader_target, &script, runner).await;
    let (status, detail) = match outcome {
        Ok(output) if output.ok() => (COMPLETED, output.stdout.trim().to_string()),
        Ok(output) => {
            let captured = json!({
                "operation": "native_reader_convergence",
                "exit_code": output.code,
                "stderr": output.stderr.trim(),
                "stdout": output.stdout.trim(),
            });
            (FAILED, captured.to_string())
        }
        Err(error) => (FAILED, error.to_string()),
    };
    pass.releases.push(Released {
        binary: "stado-readers".to_string(),
        version,
        status,
        detail,
    });
}
