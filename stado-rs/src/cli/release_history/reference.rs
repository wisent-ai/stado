//! Execute a planned branch move through its declared managed preflight,
//! or ordinary Git when no managed provider owns the operation.

use super::replay::Git;
use crate::cli::CmdError;

#[derive(serde::Deserialize)]
struct ReviewedMove<'a> {
    #[serde(borrow)]
    kind: std::borrow::Cow<'a, str>,
    #[serde(borrow)]
    reference: std::borrow::Cow<'a, str>,
    #[serde(borrow)]
    old: std::borrow::Cow<'a, str>,
    #[serde(borrow)]
    new: std::borrow::Cow<'a, str>,
}

/// The installed hook's ownership and capability headers are protocol
/// identities emitted by Tama, not a guess from its path or executable name.
/// Invoke that same dispatcher so its declared executable and environment
/// perform the preflight, rather than a different `tama` found on our PATH.
pub(super) fn move_branch(
    git: &Git,
    reference: &str,
    new: &str,
    old: &str,
) -> Result<(), CmdError> {
    let hook = git.one(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "hooks/reference-transaction",
    ])?;
    let content = match std::fs::read(&hook) {
        Ok(content) => content,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(cause) => {
            return Err(CmdError::click(format!(
                "reference hook {hook} cannot be read: {cause}"
            )));
        }
    };
    let declares = |header: &[u8]| content.split(|byte| *byte == b'\n').any(|line| line == header);
    if !declares(b"# tama managed global Git dispatcher") {
        git.one(&["update-ref", "-m", "stado release history", reference, new, old])?;
        return Ok(());
    }
    if !declares(b"# tama reference-move preflight") {
        return Err(CmdError::refused(format!(
            "Tama's reference hook {hook} declares no reference-move preflight; update its managed \
             dispatcher through Tama's supported installation before rewriting history; no branch move was started"
        )));
    }
    let mut command = std::process::Command::new(&hook);
    command
        .current_dir(&git.root)
        .args(["move", reference, new, old, "stado release history"]);
    let output = crate::wait::output(&mut command).map_err(|cause| {
        CmdError::click(format!(
            "reference preflight {hook} could not run in {}: {cause}",
            git.root.display()
        ))
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(CmdError::refused(format!(
            "reference preflight {hook} for {reference} from {old} to {new} exited {}: {}",
            output.status,
            stderr.trim()
        )));
    }
    let reviewed = serde_json::from_slice::<ReviewedMove<'_>>(&output.stdout);
    if !matches!(&reviewed, Ok(result) if result.kind == "tama reviewed reference move"
        && result.reference == reference && result.old == old && result.new == new)
    {
        let cause = match reviewed {
            Err(cause) => format!("the reviewed-move result could not be decoded: {cause}"),
            Ok(result) => format!(
                "received {:?}: {} from {} to {}",
                result.kind, result.reference, result.old, result.new
            ),
        };
        let observed = git.one(&["rev-parse", "--verify", reference]);
        return Err(CmdError::refused(format!(
            "reference preflight {hook} exited successfully but returned no matching reviewed move: \
             {cause}; observed {reference}: {observed:?}; stderr: {}",
            stderr.trim()
        )));
    }
    eprint!("{stderr}");
    Ok(())
}
