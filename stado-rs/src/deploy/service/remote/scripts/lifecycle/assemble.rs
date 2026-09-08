use crate::deploy::service::*;

/// The shared prelude with this unit spliced in: the vocabulary (`$unit`,
/// `$domain`, `$domain_status`, `$domain_reason`, `$launch`,
/// `stado_systemctl`, `say`, and the three `stado_unit_*` reads) every body and
/// every postcondition probe reads the host through.
///
/// [`DOMAIN_RESOLVER`] and [`UNIT_STATE`] are spliced here and nowhere else,
/// so no body can answer "which domain is this unit in" or "which processes
/// are this unit" for itself. Both questions used to be answered inline, per
/// body, and the answers disagreed.
///
/// `no_domain` is what the prelude does when a Darwin host has no per-login
/// launchd domain at all. It is a parameter and not a fixed refusal because
/// the two answers are genuinely different operations: everything that
/// addresses an installed unit has nothing to act on ([`NO_DOMAIN_REFUSE`]),
/// while [`ensure_service`] installs the daemon spelling into the domain that
/// does exist ([`NO_DOMAIN_SYSTEM`]).
pub(crate) fn prelude_with(
    unit: &str,
    linux_unit: &str,
    path: &str,
    no_domain: &str,
    observed_domain: Option<&str>,
) -> Result<String, DeployError> {
    validate_unit_id(unit)?;
    Ok(REMOTE_PRELUDE
        .replace("@DOMAIN_RESOLVER@", DOMAIN_RESOLVER)
        .replace(
            "@OBSERVED_DOMAIN@",
            &observed_domain.map_or_else(String::new, |domain| {
                format!(
                    "  domain={}\n  domain_status={}\n  domain_reason='the exact loaded owner was observed before this lifecycle action'\n",
                    shlex_quote(domain),
                    if domain.starts_with("gui/") { "graphical" } else { "fallback" },
                )
            }),
        )
        .replace("@UNIT_STATE@", UNIT_STATE)
        .replace("@UNIT@", &shlex_quote(unit))
        .replace("@LINUX_UNIT@", &shlex_quote(linux_unit))
        .replace("@PATH@", &quote_unit_path(path)?)
        .replace("@NO_DOMAIN@", no_domain))
}

pub(crate) fn remote_prelude(
    unit: &str,
    linux_unit: &str,
    path: &str,
) -> Result<String, DeployError> {
    prelude_with(unit, linux_unit, path, NO_DOMAIN_REFUSE, None)
}

/// Assemble a remote program: the shared prelude with this unit spliced in,
/// then one fixed body.
pub(crate) fn remote_script(
    unit: &str,
    linux_unit: &str,
    path: &str,
    body: &str,
) -> Result<String, DeployError> {
    let prelude = remote_prelude(unit, linux_unit, path)?;
    Ok(format!("{prelude}{body}"))
}

/// The shared prelude with one unit spliced in, ahead of a caller's own body.
///
/// [`crate::deploy::service_serving`] needs exactly the vocabulary every body here
/// reads the host through — `$unit`, `$unit_path`, `$domain`, `$launch`,
/// `stado_launchd_state` — and must not grow a second copy of it. A second
/// resolver for "which domain is this unit in" is how two commands come to
/// disagree about the same unit, which is the failure [`prelude_with`] exists
/// to prevent.
pub fn serving_script(unit: &str, path: &str, body: &str) -> Result<String, DeployError> {
    remote_script(unit, "", path, body)
}
