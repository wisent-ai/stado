//! Whether a candidate host can carry a Jeden session now, asked of the host
//! itself over its channel.

use super::{ledger_root, workspace_expression, MANAGED_JEDEN, MANAGED_STADO, OPERATOR_GRANT_FILE};
use crate::deploy::host_channel;
use crate::targets::ComputeTarget;

/// What a candidate host answered about carrying a session.
pub(in crate::cli::workload::runners) struct HostReadiness {
    /// True when the host holds the operator grant file Jeden reads its own
    /// credentials through. A host without it needs the fleet to hand the
    /// session its credentials as declared job secrets instead.
    pub reads_own_credentials: bool,
}

/// Whether `target` can carry a session in `checkout` right now: the
/// workspace exists, the managed binaries are installed, and the named
/// session ledger is there when one was named. `Err(sentence)` is the
/// host's own reason, kept for the refusal that names every candidate.
pub(in crate::cli::workload::runners) async fn probe_ready(
    target: &ComputeTarget,
    checkout: &str,
    resume: Option<&str>,
) -> Result<HostReadiness, String> {
    let (ledger, _) = ledger_root(target);
    let workspace = workspace_expression(checkout);
    let resume_probe = match resume {
        Some(session) => format!(
            "if [ ! -d {ledger}/{session} ]; then printf 'session ledger is missing: %s\\n' {ledger}/{session} >&2; ready=no; fi\n"
        ),
        None => String::new(),
    };
    let probe = format!(
        r#"ready=yes
if [ ! -d {workspace} ]; then
  printf 'workspace directory is missing: %s\n' {workspace} >&2
  ready=no
fi
{resume_probe}
for binary in {MANAGED_JEDEN} {MANAGED_STADO}; do
  if [ ! -x "$HOME/$binary" ]; then
    printf 'managed runtime is missing or not executable: %s\n' "$HOME/$binary" >&2
    ready=no
  fi
done
[ "$ready" = yes ] || exit 1
if [ -r "$HOME"/{OPERATOR_GRANT_FILE} ]; then
  printf 'ready own-credentials'
else
  printf 'ready fleet-credentials'
fi
"#,
    );
    let runner = crate::deploy::production_runner();
    match host_channel::run_script(target, &probe, &runner).await {
        Ok(output) if output.succeeded() && output.stdout.trim().starts_with("ready") => {
            Ok(HostReadiness {
                reads_own_credentials: output.stdout.trim().ends_with("own-credentials"),
            })
        }
        Ok(output) => {
            let detail = output.detail();
            Err(format!(
                "{}: {}",
                target.name,
                if detail.trim().is_empty() {
                    "the runtime preflight returned no readiness or failure detail"
                } else {
                    detail.trim()
                }
            ))
        }
        Err(error) => Err(format!("{}: {error}", target.name)),
    }
}
