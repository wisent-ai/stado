use std::path::Component;

use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Splicing operator data into the fixed remote programs
// ---------------------------------------------------------------------------

/// Splice a unit-file path into a fixed remote program.
///
/// Registry-declared paths use the `$HOME/...` idiom —
/// `host_recovery::MANAGED_AGENTS` spells every plist that way, and the
/// recovery script splices them inside double quotes for exactly this
/// reason — so `shlex_quote` is wrong here: it would ship a literal `$HOME`
/// and every lookup would miss. Double quotes keep the expansion, and are
/// only safe on a vetted charset, so anything that could open a command
/// substitution, escape the quotes or add a line is refused outright rather
/// than escaped into something subtle. An empty path means "let the remote
/// program derive it".
pub fn quote_unit_path(path: &str) -> Result<String, DeployError> {
    if path.is_empty() {
        return Ok(String::new());
    }
    let body = path.strip_prefix(HOME_PREFIX).unwrap_or(path);
    let safe = |ch: char| ch.is_ascii_alphanumeric() || "_-./+@:".contains(ch);
    if body.chars().all(safe) {
        return Ok(path.to_string());
    }
    Err(DeployError(format!(
        "unit path {} contains characters that cannot ride the fixed remote program",
        py_str_repr(path)
    )))
}

/// Validate one remote destination file independently of shell quoting.
///
/// The commands that write a file on a managed host deliberately support only
/// an absolute path or a path rooted at the target user's home. The value
/// travels base64-encoded, but rejecting parent traversal keeps a typo from
/// turning a credential sync into an unrelated file rewrite. `label` names the
/// destination the way the operator asked for it -- an environment file for
/// `service secret-sync`, a token file for `service token-file-sync` -- so a
/// refusal says which of a command's paths was wrong.
pub(crate) fn validate_home_rooted_file(path: &str, label: &str) -> Result<(), DeployError> {
    let local = path.strip_prefix("$HOME/").unwrap_or(path);
    let rooted = path.starts_with('/') || path.starts_with("$HOME/");
    let usable_file = Path::new(local).file_name().is_some();
    let traverses_parent = Path::new(local)
        .components()
        .any(|part| matches!(part, Component::ParentDir));
    if rooted && usable_file && !traverses_parent && !path.chars().any(char::is_control) {
        return Ok(());
    }
    Err(DeployError(format!(
        "{label} {} must be an absolute or home-relative file path without parent traversal",
        py_str_repr(path)
    )))
}

pub(crate) fn validate_env_variable(variable: &str) -> Result<(), DeployError> {
    let mut chars = variable.chars();
    let head_ok = chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_uppercase());
    let tail_ok = chars.all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit());
    if head_ok && tail_ok {
        return Ok(());
    }
    Err(DeployError(format!(
        "environment variable {} must match [A-Z_][A-Z0-9_]*",
        py_str_repr(variable)
    )))
}

pub(crate) fn validate_secret_value(value: &str) -> Result<(), DeployError> {
    if !value.is_empty() && !value.chars().any(char::is_control) {
        return Ok(());
    }
    Err(DeployError(
        "secret value must be non-empty and single-line".to_string(),
    ))
}

pub(crate) fn validate_loopback_probe_url(raw: &str) -> Result<(), DeployError> {
    let parsed = url::Url::parse(raw)
        .map_err(|error| DeployError(format!("invalid service probe URL: {error}")))?;
    let loopback = parsed
        .host_str()
        .is_some_and(|host| matches!(host, "127.0.0.1" | "localhost" | "::1"));
    if parsed.scheme() == "http"
        && loopback
        && parsed.port().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none()
    {
        return Ok(());
    }
    Err(DeployError(format!(
        "service probe URL {} must be an explicit loopback HTTP endpoint without credentials or a fragment",
        py_str_repr(raw)
    )))
}

/// A body line equal to the heredoc delimiter would end the heredoc early
/// and hand the rest of the unit to the shell as commands. Nothing this
/// crate renders contains such a line; refuse rather than assume.
pub(crate) fn guard_heredoc(content: &str) -> Result<(), DeployError> {
    if content.lines().any(|line| line.trim() == UNIT_HEREDOC) {
        return Err(DeployError(format!(
            "rendered unit contains the reserved delimiter line {}",
            py_str_repr(UNIT_HEREDOC)
        )));
    }
    Ok(())
}

/// The registry's own target-name rule, applied to a service name because
/// the name becomes part of a launchd label, part of a systemd unit name,
/// and a field of the canonical document. Mirrors the check
/// `targets.rs::validate_registry` runs on `registry.targets[].name`.
pub(crate) fn validate_service_name(name: &str) -> Result<(), DeployError> {
    let inner = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ".-_".contains(ch);
    let edge = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit();
    let head_ok = name.chars().next().is_some_and(edge);
    let tail_ok = name.chars().next_back().is_some_and(edge);
    if head_ok && tail_ok && name.chars().all(inner) {
        return Ok(());
    }
    Err(DeployError(format!(
        "service name {} must be a lowercase identifier of letters, digits, '.', '-' and '_'",
        py_str_repr(name)
    )))
}

/// The program a deployed unit runs. It is interpolated raw into the plist
/// (`local_install::plist_text` does no XML escaping, matching the Python
/// it was ported from) and into the systemd `ExecStart`, so it has to be
/// well-formed for both without escaping.
pub(crate) fn validate_program(program: &str) -> Result<(), DeployError> {
    if !program.starts_with('/') {
        return Err(DeployError(format!(
            "--from {} must be an absolute path on the target host",
            py_str_repr(program)
        )));
    }
    if program
        .chars()
        .any(|ch| ch.is_control() || "<>&\"'".contains(ch))
    {
        return Err(DeployError(format!(
            "--from {} contains characters that cannot be rendered into a unit file",
            py_str_repr(program)
        )));
    }
    Ok(())
}

/// An argument the deployed unit is started with. It lands in the same two
/// places as the program and under the same no-escaping rule, and a unit
/// whose arguments are empty strings is a unit nobody can read back from
/// `service show`, so both are refused here rather than at the host.
pub(crate) fn validate_unit_argument(arg: &str) -> Result<(), DeployError> {
    if arg.is_empty() {
        return Err(DeployError(
            "--arg cannot be empty; drop it instead".to_string(),
        ));
    }
    if arg
        .chars()
        .any(|ch| ch.is_control() || "<>&\"'".contains(ch))
    {
        return Err(DeployError(format!(
            "--arg {} contains characters that cannot be rendered into a unit file",
            py_str_repr(arg)
        )));
    }
    Ok(())
}

/// Reject a unit id that cannot ride the remote program as a shell word.
/// `shlex_quote` handles the quoting, but a control character in a launchd
/// label is never a real unit and would corrupt the marker framing.
pub fn validate_unit_id(unit: &str) -> Result<(), DeployError> {
    if unit.is_empty() || unit.chars().any(char::is_control) {
        return Err(DeployError(format!(
            "unit {} is not a usable launchd label or systemd unit name",
            py_str_repr(unit)
        )));
    }
    Ok(())
}
