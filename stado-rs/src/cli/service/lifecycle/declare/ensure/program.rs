//! What a unit runs, and which declaration said so.

use super::*;

/// What a unit runs, and which declaration said so.
///
/// `pub(crate)` because the autonomy service reconciler renders repair units
/// through this exact chain; a second resolution order over there is how one
/// unit gets two different programs depending on who asked.
pub(crate) struct UnitProgram {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    /// `"flag"`, `"registry"`, `"catalog"` or `"shipped"`.
    pub(crate) source: &'static str,
    /// Stable unit identity supplied by a registry or catalog declaration.
    pub(crate) unit: Option<String>,
    /// Non-secret environment declared by this unit, with target placeholders intact.
    pub(crate) env: std::collections::BTreeMap<String, String>,
    /// Exact authored systemd definition retained by registry lifecycle repairs.
    pub(crate) systemd_unit: String,
}
pub(crate) fn declared_label(service: &ManagedService) -> Option<&str> {
    let unit_id = service.unit_id();
    Some(unit_id.strip_suffix(".service").unwrap_or(unit_id)).filter(|label| !label.is_empty())
}

/// Stable unit identity supplied by the managed-product declaration.
///
/// Service names are operator-facing leaves (`stado-resolver`), while the
/// product catalog carries the init system's full identity
/// (`com.wisent.stado-resolver`). A registry record may contain the broken
/// historical spelling, so it cannot be the authority for this lookup.
pub(super) fn canonical_managed_unit(name: &str, target: &str) -> Result<Option<String>, CmdError> {
    let requested = name.strip_suffix(".service").unwrap_or(name);
    let mut matched: Option<String> = None;
    let products = crate::deploy::products::declared().map_err(click)?;
    for product in products {
        for unit in &product.units {
            let label = unit.label_for(target);
            let bare = label.strip_suffix(".service").unwrap_or(&label);
            let leaf = bare.rsplit('.').next().unwrap_or(bare);
            if label != name && bare != requested && leaf != requested {
                continue;
            }
            if matched.as_deref().is_some_and(|existing| existing != label) {
                return Err(CmdError::click(format!(
                    "managed product declarations give {name} more than one unit identity"
                )));
            }
            matched = Some(label);
        }
    }
    Ok(matched)
}

/// The program and argument vector `ensure` renders the unit from.
///
/// Flags win, because an operator correcting a wrong declaration has to be
/// able to. Absent them, the host's own `services[]` entry answers: a
/// declaration that carries its program is one this command can reinstall
/// from the document alone, which is the whole difference between a declared
/// service and a plist somebody installed by hand — the resolver and the
/// local dashboard on `operator-host` were the second kind, and nothing in
/// the product knew their restart policy. Last, the declaration shipped in
/// this build ([`targets::load_bundled_registry`]), which is how a unit
/// declared in a release reaches a canonical document published before it:
/// the first `ensure` writes it there.
pub(crate) fn unit_program(
    host: &str,
    name: &str,
    from: Option<&str>,
    args: &[String],
    declared: Option<&ManagedService>,
) -> Result<UnitProgram, CmdError> {
    if let Some(from) = from {
        return Ok(UnitProgram {
            program: from.to_string(),
            args: args.to_vec(),
            source: "flag",
            unit: None,
            env: declared
                .map(|service| service.env.clone())
                .unwrap_or_default(),
            systemd_unit: declared
                .map(|service| service.systemd_unit.clone())
                .unwrap_or_default(),
        });
    }
    if !args.is_empty() {
        return Err(CmdError::usage(
            "--arg needs --from: an argument vector without the program it belongs to would be \
             appended to a declared program the caller never named",
        ));
    }
    if let Some(declared) = declared.filter(|candidate| !candidate.program.is_empty()) {
        return Ok(UnitProgram {
            program: declared.program.clone(),
            args: declared.args.clone(),
            source: "registry",
            unit: Some(declared.unit_id().to_string()),
            env: declared.env.clone(),
            systemd_unit: declared.systemd_unit.clone(),
        });
    }
    // The shipped Wisent catalog answers by name, on any host, with no
    // declaration of the operator's own — that is the whole of "run Weles
    // here" as one word.
    if let Some(entry) = crate::deploy::service_catalog::lookup(name)
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Ok(UnitProgram {
            // Placeholders survive here on purpose: `$HOME` and
            // `$STADO_PLATFORM` belong to the target, and only the caller
            // holding the resolved target may expand them.
            program: entry.program,
            args: entry.args,
            source: "catalog",
            unit: entry.unit,
            env: entry.env,
            systemd_unit: String::new(),
        });
    }
    let bundled =
        targets::load_bundled_registry().map_err(|error| CmdError::click(error.to_string()))?;
    let shipped = bundled
        .lookup(host)
        .map(service::declared_services)
        .unwrap_or_default()
        .into_iter()
        .find(|candidate| candidate.matches(name) && !candidate.program.is_empty());
    if let Some(shipped) = shipped {
        let unit = shipped.unit_id().to_string();
        return Ok(UnitProgram {
            program: shipped.program,
            args: shipped.args,
            source: "shipped",
            unit: Some(unit),
            env: shipped.env,
            systemd_unit: shipped.systemd_unit,
        });
    }
    Err(CmdError::usage(format!(
        "nothing declares what {name} runs on {host}: pass --from PATH (repeating --arg for each \
         argument), give its registry services[] entry a \"program\" and \"args\", or pick one of \
         the preconfigured Wisent services `stado service catalog` lists"
    )))
}
