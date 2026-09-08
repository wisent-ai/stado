//! Move a delivered policy into the path the worker reads, or refuse and
//! change nothing.

use super::jq::{jq_eval, POLICY_JQ_FILTER, POLICY_JQ_SUMMARIZE};
use super::{POLICY_FILE, POLICY_MARKER, VANTAGE_MARKER};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Move the registry-published Weles placement policy into the path the worker
/// reads, or refuse and change nothing — the checks the retired apply script
/// ran, as individual remote commands with every branch taken here.
///
/// `stado route placement publish` delivers the document to
/// `$HOME/.stado/files/placement-policy.json` through the audited channel and
/// then runs this. It takes no operator input on purpose: a writer that
/// accepted a source or a destination path would be a remote writer with the
/// audit trail removed. Both paths are fixed, so the only thing an operator
/// can vary is what the registry says.
///
/// Three refusals, all of them silent failures somewhere else:
///
///   not JSON      a truncated or half-written delivery. Installing it takes
///                 the worker's placement loader out entirely, on every claim.
///   no _source    an unstamped document is one nobody can trace to a registry
///                 read. That is the file this whole change exists to retire:
///                 the host copy that disagreed with the registry for hours
///                 and could not be dated, attributed, or compared against it.
///   not this host a policy whose entries name no identity of this machine
///                 does not fail loudly in the worker. It resolves to
///                 `enabled: false` and the worker declines every row in
///                 silence -- 29,616 times, the last time this fleet learned it.
///
/// The destination is written through a temporary file in the same directory
/// and renamed, so a worker reading concurrently sees either the whole old
/// document or the whole new one, never a partial write.
///
/// The return is the report the retired script printed, composed here from
/// what the host answered: the `PLACEMENT_VANTAGE` and `PLACEMENT_POLICY`
/// marker lines and the closing human line, so
/// [`snapshot`](super::receipt::snapshot) and the vantage read below parse it
/// unchanged.
pub(super) async fn apply_policy(
    resolved: &ComputeTarget,
    runner: &Runner,
) -> Result<String, DeployError> {
    let home = host_channel::remote_home(resolved, runner).await?;
    let source = format!("{home}/.stado/files/{POLICY_FILE}");
    let dest_dir = format!("{home}/.config/weles");
    let dest = format!("{dest_dir}/{POLICY_FILE}");

    // Every check below is a JSON question, so a host without jq cannot answer
    // any of them -- and a writer that cannot verify must not write. jq is at
    // /usr/bin on the Linux hosts and under Homebrew on the macOS ones, so the
    // three places it is actually installed are tried rather than one assumed.
    let mut jq = None;
    for candidate in ["/usr/bin/jq", "/opt/homebrew/bin/jq", "/usr/local/bin/jq"] {
        if host_channel::remote_test(resolved, &format!("-x {candidate}"), runner).await? {
            jq = Some(candidate);
            break;
        }
    }
    let Some(jq) = jq else {
        return Err(DeployError(
            "no jq on this host: refusing to install a placement policy nothing here can parse"
                .to_string(),
        ));
    };

    // `hostname` and node's `os.hostname()` are both gethostname(2), so this
    // compares the same string the worker's loader will compare.
    let looked_up = host_channel::run_program(resolved, &["/bin/hostname"], runner).await?;
    let host = looked_up.stdout.trim().to_string();
    if host.is_empty() {
        return Err(DeployError(
            "this host cannot state its own hostname; the worker resolves placement by it"
                .to_string(),
        ));
    }

    let refuse = |why: &str| DeployError(format!("refusing to install {source}: {why}"));
    let quoted_source = crate::deploy::shlex_quote(&source);
    if !host_channel::remote_test(resolved, &format!("-f {quoted_source}"), runner).await? {
        return Err(refuse("no delivered document at that path"));
    }
    if !jq_eval(
        resolved,
        runner,
        jq,
        None,
        false,
        r#"type == "object""#,
        &source,
    )
    .await?
    .ok()
    {
        return Err(refuse("it does not parse as a JSON object"));
    }
    if !jq_eval(
        resolved,
        runner,
        jq,
        None,
        false,
        r#"(._source | type) == "object"
  and ((._source.registry_generation // "") | tostring | length) > 0
  and ((._source.published_at // "") | tostring | length) > 0
  and ((._source.by // "") | tostring | length) > 0"#,
        &source,
    )
    .await?
    .ok()
    {
        return Err(refuse(
            "it carries no _source stamp naming the registry generation it came from",
        ));
    }
    if !jq_eval(
        resolved,
        runner,
        jq,
        None,
        false,
        r#".schema_version == 1 and (.hosts | type) == "array""#,
        &source,
    )
    .await?
    .ok()
    {
        return Err(refuse(
            "the worker parses schema_version 1 with a hosts array, and this is not that",
        ));
    }
    let entry_query = format!("{POLICY_JQ_FILTER}\n($host | norm) as $h | entry($h) != null");
    if !jq_eval(
        resolved,
        runner,
        jq,
        Some(host.as_str()),
        false,
        &entry_query,
        &source,
    )
    .await?
    .ok()
    {
        return Err(refuse(&format!(
            "no entry names this host ({host}), so the worker would silently refuse every action"
        )));
    }

    let summarize_query = format!("{POLICY_JQ_FILTER}\n{POLICY_JQ_SUMMARIZE}");

    // Read what is already there BEFORE overwriting it: after the rename
    // nothing on this machine can still say what the host was running on.
    let quoted_dest = crate::deploy::shlex_quote(&dest);
    if host_channel::remote_test(resolved, &format!("-L {quoted_dest}"), runner).await? {
        return Err(DeployError(format!(
            "refusing to write through a symlink: {dest}"
        )));
    }
    let previous =
        if !host_channel::remote_test(resolved, &format!("-e {quoted_dest}"), runner).await? {
            "absent\t-\t-".to_string()
        } else {
            let summarized = jq_eval(
                resolved,
                runner,
                jq,
                Some(host.as_str()),
                true,
                &summarize_query,
                &dest,
            )
            .await?;
            match summarized
                .ok()
                .then(|| summarized.stdout.trim().to_string())
            {
                Some(line) if !line.is_empty() => line,
                _ => "unreadable\t-\t-".to_string(),
            }
        };

    let made =
        host_channel::run_program(resolved, &["/bin/mkdir", "-p", &dest_dir], runner).await?;
    if !made.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &made,
            "could not create the policy directory",
        )));
    }
    let temporary = format!(
        "{dest_dir}/.placement-policy.json.stado-apply-{}",
        std::process::id()
    );
    for words in [
        vec!["/bin/cp", source.as_str(), temporary.as_str()],
        vec!["/bin/chmod", "600", temporary.as_str()],
        vec!["/bin/mv", temporary.as_str(), dest.as_str()],
    ] {
        let stepped = host_channel::run_program(resolved, &words, runner).await?;
        if !stepped.ok() {
            // A failed install leaves no half-written destination and no
            // staging litter: the temporary file goes, exactly as the retired
            // script's EXIT trap removed it.
            let _ =
                host_channel::run_program(resolved, &["/bin/rm", "-f", &temporary], runner).await;
            return Err(DeployError(host_channel::last_error_line(
                &stepped,
                "remote install failed",
            )));
        }
    }

    let installed_output = jq_eval(
        resolved,
        runner,
        jq,
        Some(host.as_str()),
        true,
        &summarize_query,
        &dest,
    )
    .await?;
    let installed = installed_output.stdout.trim().to_string();
    if !installed_output.ok() || installed.is_empty() {
        return Err(DeployError(host_channel::last_error_line(
            &installed_output,
            "the installed policy could not be read back",
        )));
    }
    let generation = installed.split('\t').next().unwrap_or_default();

    Ok(format!(
        "{VANTAGE_MARKER}\t{host}\n\
         {POLICY_MARKER}\tprevious\t{previous}\n\
         {POLICY_MARKER}\tinstalled\t{installed}\n\
         installed {dest} at registry generation {generation}\n"
    ))
}
