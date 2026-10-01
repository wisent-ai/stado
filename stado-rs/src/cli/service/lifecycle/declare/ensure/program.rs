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
            let bare = label.strip_suffix(".service").unwrap_or(&label).to_string();
            let leaf = bare.rsplit('.').next().unwrap_or(&bare);
            if label != name && bare != requested && leaf != requested {
                continue;
            }
            // A product declares a unit once per init system: `com.wisent.stado`
            // for launchd and `com.wisent.stado.service` for systemd are one
            // unit. Compared whole, they refused `service ensure` for the host
            // Stado process with "more than one unit identity".
            if matched.as_deref().is_some_and(|existing| existing != bare) {
                return Err(CmdError::click(format!(
                    "managed product declarations give {name} more than one unit identity"
                )));
            }
            matched = Some(bare);
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

/// The program a pass renders and the environment it runs with.
///
/// The catalog's environment is the product's own requirement for the unit,
/// so it applies whatever declared the program: a registry entry adopted from
/// a hand-installed plist names the same binary and still needs the same
/// variables. Program and args keep [`unit_program`]'s resolution order; only
/// the environment is defaulted from the catalog, then the unit's own
/// declaration and `--env` override it, each value resolved for this target.
pub(super) fn resolved_unit(
    target: &crate::targets::ComputeTarget,
    options: &EnsureOptions<'_>,
    existing: Option<&ManagedService>,
    catalog_entry: Option<&crate::deploy::service_catalog::CatalogService>,
) -> Result<(UnitProgram, Vec<(String, String)>), CmdError> {
    let host = target.name.as_str();
    let home = crate::deploy::service_catalog::home_for(target);
    let mut unit_env: Vec<(String, String)> = catalog_entry
        .map(|entry| {
            crate::deploy::service_catalog::resolve_entry(
                entry,
                &home,
                Some(&target.release_platform),
                &target.name,
            )
            .2
        })
        .unwrap_or_default();
    let mut unit = unit_program(host, options.name, options.from, options.args, existing)?;
    if unit.source == "catalog" {
        let entry = crate::deploy::service_catalog::CatalogService {
            name: options.name.to_string(),
            summary: String::new(),
            unit: unit.unit.clone(),
            program: unit.program.clone(),
            args: unit.args.clone(),
            env: unit.env.clone(),
            retired_units: Vec::new(),
            role_units: Vec::new(),
        };
        let (program, args, env) = crate::deploy::service_catalog::resolve_entry(
            &entry,
            &home,
            Some(&target.release_platform),
            &target.name,
        );
        unit.program = program;
        unit.args = args;
        unit_env = env;
        eprintln!(
            "{host} declares no program for {}; rendering the unit from the Wisent service \
             catalog this build ships: {} {}",
            options.name,
            unit.program,
            unit.args.join(" ")
        );
    }
    if unit.source == "shipped" {
        eprintln!(
            "{host} declares no program for {}; rendering the unit from the declaration shipped \
             with this build: {} {}",
            options.name,
            unit.program,
            unit.args.join(" ")
        );
    }
    let mut env_overrides = std::mem::take(&mut unit.env);
    for assignment in options.env {
        let (name, value) = assignment
            .split_once('=')
            .ok_or_else(|| CmdError::usage("--env requires NAME=VALUE"))?;
        env_overrides.insert(name.to_string(), value.to_string());
    }
    for (name, value) in env_overrides {
        let value = crate::deploy::service_catalog::resolve_word(
            &value,
            &home,
            Some(&target.release_platform),
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
