//! The command a release build job runs on its builder before the worker
//! itself starts, and after the worker exits.

mod head;
mod tail;

/// Run a release worker from the queue-owned persistent work root even when an
/// older agent materialized this job under `/tmp`.
///
/// The bridge accepts only the physical forms of the two paths the old and new
/// agents can assign to this exact canonical job id. It moves the
/// already-materialized tree before the worker starts. A compatibility symlink
/// lets the old agent's heartbeat and terminal uploader follow their hard-coded
/// path. After the worker exits, successful output is uploaded and read back
/// through the checked storage writer at both canonical status and exact
/// attempt URIs, with the attempt receipt last; any failure changes the
/// workload result to failure. A lifecycle-bound keeper uses the 0.15.10
/// `job watch --follow --json` contract: that command emits `"terminal": true`
/// even when it returns failure for a failed job. The keeper persists and
/// inspects that exact response before unlinking the verified symlink, and also
/// removes that link if the persistent workdir disappears, so an unbounded
/// artifact-finalization retry remains reachable without leaving a permanent
/// process or dangling link. Explicit log redirection and TMPDIR keep both
/// diagnostics and the old worker's build scratch in the persistent tree even
/// when `/tmp` is a separate filesystem.
const RELEASE_OUTPUT_URI_MARK: &str = "@RELEASE_OUTPUT_URI@";

/// The bootstrap command for one exact attempt output coordinate.
pub(super) fn release_worker_command(output_uri: &str) -> String {
    let mut command = String::from(head::RELEASE_WORKER_COMMAND_HEAD);
    command.push_str(tail::RELEASE_WORKER_COMMAND_TAIL);
    command.replace(
        RELEASE_OUTPUT_URI_MARK,
        &crate::deploy::shlex_quote(output_uri),
    )
}
