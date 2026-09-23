//! Convert a captured native definition without consulting installer defaults.

use std::path::Path;

use crate::deploy::local_install::{systemd_unit, InstallPlan, LocalOs};
use crate::deploy::service::{parse_local_unit_file, UnitFile, KIND_LAUNCHD, KIND_SYSTEMD};
use crate::deploy::DeployError;

use super::Component;

impl Component {
    /// `plan.name` identifies the host; its label and execution domain identify
    /// the native unit. The captured file supplies argv, environment and cadence.
    pub(crate) fn from_definition(
        mut plan: InstallPlan,
        definition: UnitFile,
    ) -> Result<Self, DeployError> {
        if definition.host != plan.name {
            return Err(DeployError(format!(
                "{} was captured on {}, not planned host {}",
                definition.unit, definition.host, plan.name
            )));
        }
        let expected_kind = match plan.os {
            LocalOs::Darwin => KIND_LAUNCHD,
            LocalOs::Linux => KIND_SYSTEMD,
        };
        if definition.kind != expected_kind {
            return Err(DeployError(format!(
                "{}: native kind {} differs from planned kind {expected_kind}",
                definition.unit, definition.kind
            )));
        }
        let expected_unit = match plan.os {
            LocalOs::Darwin => plan.label.clone(),
            LocalOs::Linux => systemd_unit(&plan.label),
        };
        if definition.unit != expected_unit {
            return Err(DeployError(format!(
                "captured native unit {} differs from planned unit {expected_unit}",
                definition.unit
            )));
        }
        let parsed =
            parse_local_unit_file(&definition.content, definition.kind).map_err(|error| {
                DeployError(format!(
                    "{} at {}: {error}",
                    definition.unit, definition.path
                ))
            })?;
        if parsed.start_commands != 1 || parsed.program.is_empty() {
            return Err(DeployError(format!(
                "{} requires exactly one native executable; observed {} start commands",
                definition.unit, parsed.start_commands
            )));
        }
        if !parsed.unresolved_expansions.is_empty() {
            return Err(DeployError(format!(
                "{} has unresolved native substitutions in {}; capture effective values before consolidation",
                definition.unit, parsed.unresolved_expansions.join(", ")
            )));
        }
        if !parsed.environment_files.is_empty() {
            return Err(DeployError(format!(
                "{} still has unresolved EnvironmentFile declarations: {}",
                definition.unit,
                parsed.environment_files.join(", ")
            )));
        }
        let expected_program = plan
            .exec_args
            .first()
            .ok_or_else(|| DeployError(format!("{} has no declared executable", plan.label)))?;
        if Path::new(&parsed.program).file_name() != Path::new(expected_program).file_name() {
            return Err(DeployError(format!(
                "{} executes {}, not the declared component {}",
                definition.unit, parsed.program, expected_program
            )));
        }
        if definition.kind == KIND_SYSTEMD && parsed.arguments.first() != Some(&parsed.program) {
            return Err(DeployError(format!(
                "{} uses native execution modifiers that the host declaration does not preserve",
                definition.unit
            )));
        }
        plan.exec_args = parsed.arguments;
        // launchd's Program overrides the spelling of argv[0]. clap needs the
        // executable followed by its options, not the custom process title.
        plan.exec_args[0] = parsed.program;
        plan.env = parsed.env.into_iter().collect();
        Ok(Self {
            plan,
            definition,
            periodic: parsed.start_interval_seconds,
        })
    }

    pub(crate) fn native_definition(&self) -> &UnitFile {
        &self.definition
    }

    /// A continuous native owner supplies account, working directory and limits.
    /// Periodic-only definitions cannot supply the lifetime of the resident host.
    pub(crate) fn render_startup(&self, host: &InstallPlan) -> Result<String, DeployError> {
        if self.plan.os != host.os || self.plan.daemon != host.daemon || self.plan.name != host.name
        {
            return Err(DeployError(
                "resident owner differs from the captured host or execution domain".to_string(),
            ));
        }
        if host.os == LocalOs::Linux {
            return crate::deploy::service::rewrite_systemd_startup(
                &self.definition.content,
                &super::super::render::systemd_command(&host.exec_args),
                &host.env,
            );
        }
        let document = crate::deploy::service::parse_plist(&self.definition.content)?;
        if self.periodic.is_some() || document.contains_key("StartCalendarInterval") {
            return Err(DeployError(format!(
                "{} is periodic and cannot supply the resident host's lifetime",
                self.definition.unit
            )));
        }
        crate::deploy::service::rewrite_plist_startup(
            document,
            &host.label,
            &host.exec_args,
            &host.env,
        )
    }
}
