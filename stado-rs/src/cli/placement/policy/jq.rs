//! The jq vocabulary the host answers policy questions with, and the single
//! call that asks them.

use crate::deploy::{host_channel, CommandOutput, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The jq vocabulary [`apply_policy`](super::install::apply_policy) verifies
/// and summarizes the policy with: the worker's own identity rule, transcribed
/// from weles `src/worker/identity.ts` (trim, lowercase, drop trailing dots),
/// and the loader's entry-matching rule. A jq program is data for the remote
/// jq, not a shell payload; the shell never sees it.
///
/// Transcribed rather than approximated because it is a comparison, and a
/// comparison the two sides perform differently is a host that matches nothing.
pub(super) const POLICY_JQ_FILTER: &str = r#"def norm: ascii_downcase | sub("\\.+$"; "");
def entry($h): [ .hosts[]? | select(((.hostname // "") | tostring | norm) == $h
    or (((.aliases // []) | map(tostring | norm)) | index($h) != null)) ] | .[0];"#;

/// generation, enabled, actions — as three tab-separated fields, for whichever
/// entry belongs to the host named by `--arg host`. `stado host
/// publish-placement-policy` parses these to report the delta; an operator
/// reading the remote output sees them directly.
pub(super) const POLICY_JQ_SUMMARIZE: &str = r#"($host | norm) as $h
| entry($h) as $e
| [ ((._source.registry_generation // "unstamped") | tostring),
    (if $e == null then "-" else ($e.enabled | tostring) end),
    (if $e == null then "-"
     elif (($e.actions // []) | length) == 0 then "-"
     else (($e.actions | map(tostring)) | join(",")) end) ]
| @tsv"#;

/// One jq question about one JSON file on the host.
///
/// `raw` selects `-r` (the summarizing reads) over `-e` (the verifications,
/// where the exit status is the answer and the output is nothing). `host`
/// binds jq's `$host` for the entry-matching queries.
pub(super) async fn jq_eval(
    target: &ComputeTarget,
    runner: &Runner,
    jq: &str,
    host: Option<&str>,
    raw: bool,
    query: &str,
    path: &str,
) -> Result<CommandOutput, DeployError> {
    let mut words: Vec<&str> = vec![jq, if raw { "-r" } else { "-e" }];
    if let Some(host) = host {
        words.extend(["--arg", "host", host]);
    }
    words.extend([query, path]);
    host_channel::run_program(target, &words, runner).await
}
