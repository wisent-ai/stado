//! What a unit runs, and which declaration said so.

mod names;

use super::*;

pub(super) use names::canonical_unit;
pub(crate) use names::declared_label;

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
    /// Exact authored systemd definition retained by registry lifecycle repairs;
    /// empty when the declaration authored none.
    pub(crate) systemd_unit: String,
}

/// The program and argument vector `ensure` renders the unit from.
///
/// Flags win, because an operator correcting a wrong declaration has to be
/// able to. Absent them, the host's own `services[]` entry answers: a
/// declaration that carries its program is one this command can reinstall
/// from the document alone, which is the whole difference between a declared
/// service and a unit somebody installed by hand, whose restart policy
/// nothing in the product knows. Last, the declaration shipped in this build
/// ([`targets::load_bundled_registry`]), which is how a unit declared in a
/// release reaches a canonical document published before it: the first
/// `ensure` writes it there.
fn unit_program(
    host: &str,
    name: &str,
    from: Option<&str>,
    args: &[String],
    declared: Option<&ManagedService>,
) -> Result<UnitProgram, CmdError> {
    if let Some(from) = from {
        // A flag names the program; the declaration, when there is one,
        // still says the environment and any authored systemd unit.
        let (env, systemd_unit) = match declared {
            Some(service) => (service.env.clone(), service.systemd_unit.clone()),
            None => (std::collections::BTreeMap::new(), String::new()),
        };
        return Ok(UnitProgram {
            program: from.to_string(),
            args: args.to_vec(),
            source: "flag",
            unit: None,
            env,
            systemd_unit,
        });
    }
    if !args.is_empty() {
        return Err(CmdError::usage(
            "--arg needs --from: an argument vector without the program it belongs to would be \
             appended to a declared program the caller never named",
        ));
    }
    // The product catalog owns the command line of its canonical unit. A
    // registry record describes what was last installed, not a permanent
    // override of a later release's parser. Keep the explicit --from/--arg
    // escape above for an operator correcting a declaration.
    if let Some(declared) = declared {
        if let Some(entry) = crate::deploy::service_catalog::lookup(name)
            .map_err(CmdError::declaration)?
            .filter(|entry| {
                entry.unit.as_deref() == declared_label(declared)
                    && crate::deploy::service_catalog::executable_name(&entry.program)
                        == crate::deploy::service_catalog::executable_name(&declared.program)
            })
        {
            let mut env = entry.env;
            env.extend(declared.env.clone());
            return Ok(UnitProgram {
                program: entry.program,
                args: crate::cli::integrations::runtime::serve_line(
                    name,
                    entry.args,
                    &declared.args,
                )
                .map_err(CmdError::declaration)?,
                source: "catalog",
                unit: entry.unit,
                env,
                systemd_unit: String::new(),
            });
        }
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
    if let Some(entry) = crate::deploy::service_catalog::lookup(name).map_err(|error| {
        CmdError::click(error).stating(crate::primitives::failure::FailureCode::Config)
    })? {
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
    let bundled = targets::load_bundled_registry().map_err(CmdError::from)?;
    // A host the bundled registry does not carry ships no declaration.
    let shipped = match bundled.lookup(host) {
        Some(bundled_host) => service::declared_services(bundled_host)
            .into_iter()
            .find(|candidate| candidate.matches(name) && !candidate.program.is_empty()),
        None => None,
    };
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

/// The program a pass renders and the environment it runs with.
///
/// The catalog's environment is the product's own requirement for the unit,
/// so it applies whatever declared the program: a registry entry adopted from
/// a hand-installed plist names the same binary and still needs the same
/// variables. Program and args keep [`unit_program`]'s resolution order; only
/// the environment starts from the catalog's, then the unit's own
/// declaration and `--env` override it, each value resolved for this target.
/// `listen_port` is the port the service directory records for the service on
/// this target (`cli::directory::listen_port_for`).
pub(crate) fn resolved_unit(
    target: &crate::targets::ComputeTarget,
    name: &str,
    from: Option<&str>,
    args: &[String],
    existing: Option<&ManagedService>,
    catalog_entry: Option<&crate::deploy::service_catalog::CatalogService>,
    listen_port: Option<u16>,
) -> Result<(UnitProgram, Vec<(String, String)>), CmdError> {
    let host = target.name.as_str();
    let home = crate::deploy::service_catalog::home_for(target);
    let platform = target.release_platform.as_str();
    // A service the catalog does not carry brings no catalog environment.
    let mut unit_env: Vec<(String, String)> = match catalog_entry {
        Some(entry) => {
            crate::deploy::service_catalog::resolve_entry(entry, &home, platform, host, listen_port)
                .2
        }
        None => Vec::new(),
    };
    let mut unit = unit_program(host, name, from, args, existing)?;
    if unit.source == "catalog" {
        let entry = crate::deploy::service_catalog::CatalogService {
            name: name.to_string(),
            summary: String::new(),
            unit: unit.unit.clone(),
            program: unit.program.clone(),
            args: unit.args.clone(),
            env: unit.env.clone(),
            acquisition_scopes: None,
            directory_service: None,
            answering: None,
        };
        let (program, args, env) = crate::deploy::service_catalog::resolve_entry(
            &entry,
            &home,
            platform,
            host,
            listen_port,
        );
        unit.program = program;
        unit.args = args;
        unit_env = env;
        eprintln!(
            "{host} declares no program for {name}; rendering the unit from the Wisent service \
             catalog this build ships: {} {}",
            unit.program,
            unit.args.join(" ")
        );
    }
    if unit.source == "shipped" {
        eprintln!(
            "{host} declares no program for {name}; rendering the unit from the declaration \
             shipped with this build: {} {}",
            unit.program,
            unit.args.join(" ")
        );
    }
    for (variable, value) in std::mem::take(&mut unit.env) {
        let value = crate::deploy::service_catalog::resolve_word(&value, &home, platform, host);
        match unit_env.iter_mut().find(|(key, _)| key == &variable) {
            Some((_, current)) => *current = value,
            None => unit_env.push((variable, value)),
        }
    }
    Ok((unit, unit_env))
}

/// [`resolved_unit`] for one `service ensure` pass: its `--env` assignments
/// override the declared environment, and its `--unset-env` names leave it.
pub(super) fn ensured_unit(
    target: &crate::targets::ComputeTarget,
    options: &EnsureOptions<'_>,
    existing: Option<&ManagedService>,
    catalog_entry: Option<&crate::deploy::service_catalog::CatalogService>,
    listen_port: Option<u16>,
) -> Result<(UnitProgram, Vec<(String, String)>), CmdError> {
    let mut assigned = Vec::new();
    for assignment in options.env {
        let (name, value) = assignment
            .split_once('=')
            .ok_or_else(|| CmdError::usage("--env requires NAME=VALUE"))?;
        assigned.push((name.to_string(), value.to_string()));
    }
    let (unit, mut unit_env) = resolved_unit(
        target,
        options.name,
        options.from,
        options.args,
        existing,
        catalog_entry,
        listen_port,
    )?;
    let home = crate::deploy::service_catalog::home_for(target);
    for (name, value) in assigned {
        let value = crate::deploy::service_catalog::resolve_word(
            &value,
            &home,
            &target.release_platform,
            &target.name,
        );
        match unit_env.iter_mut().find(|(key, _)| key == &name) {
            Some((_, current)) => *current = value,
            None => unit_env.push((name, value)),
        }
    }
    unit_env.retain(|(name, _)| !options.unset_env.iter().any(|withdrawn| withdrawn == name));
    Ok((unit, unit_env))
}
