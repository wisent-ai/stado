use super::*;

pub(super) async fn registry_services(
    storage_target: &crate::targets::ComputeTarget,
    resident_owner_unit: &str,
    runner: &Runner,
) -> Result<Vec<ServiceCandidate>, DeployError> {
    let mut candidates = BTreeMap::<String, ServiceCandidate>::new();
    for declared in service::declared_services(storage_target) {
        if declared.unit_id() == resident_owner_unit {
            continue;
        }
        let storage_evidence = declared
            .env
            .keys()
            .filter(|key| storage_route_key(key))
            .cloned()
            .collect();
        candidates.insert(
            declared.unit_id().to_string(),
            ServiceCandidate {
                target: storage_target.clone(),
                observed_command: std::iter::once(declared.program.as_str())
                    .chain(declared.args.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" "),
                declared,
                loaded_domains: Vec::new(),
                storage_evidence,
            },
        );
    }
    for product in crate::deploy::products::declared()? {
        if !storage_target.managed_versions.contains_key(&product.name) {
            continue;
        }
        for unit in &product.units {
            let label = unit.label_for(&storage_target.name);
            if label == resident_owner_unit {
                continue;
            }
            let Some(path) = unit.path_for(&storage_target.name) else {
                continue;
            };
            let kind = unit.kind.as_deref().unwrap_or(service::KIND_LAUNCHD);
            candidates
                .entry(label.clone())
                .or_insert_with(|| ServiceCandidate {
                    target: storage_target.clone(),
                    declared: managed_from_unit(storage_target, &label, &path, kind),
                    loaded_domains: Vec::new(),
                    observed_command: String::new(),
                    storage_evidence: BTreeSet::new(),
                });
        }
    }
    let mut resident_owner_discovered = false;
    for native in service::loaded_units(storage_target, runner).await? {
        let label = native.label.clone();
        if label == resident_owner_unit {
            if native.pid.parse::<u32>().ok() != Some(std::process::id()) {
                return Err(DeployError(
                    "loaded-unit scan did not bind the exact resident owner service to this process"
                        .to_string(),
                ));
            }
            resident_owner_discovered = true;
            candidates.remove(&label);
            continue;
        }
        let candidate = candidates.entry(label.clone()).or_insert_with(|| {
            let kind = if native.path.ends_with(".service") {
                service::KIND_SYSTEMD
            } else {
                service::KIND_LAUNCHD
            };
            ServiceCandidate {
                target: storage_target.clone(),
                declared: managed_from_unit(storage_target, &label, &native.path, kind),
                loaded_domains: Vec::new(),
                observed_command: String::new(),
                storage_evidence: BTreeSet::new(),
            }
        });
        for key in native
            .env_keys
            .iter()
            .chain(&native.script_reads)
            .chain(&native.script_assigns)
            .filter(|key| storage_route_key(key))
        {
            candidate.storage_evidence.insert(key.clone());
        }
        if candidate.declared.path.is_empty() && !native.path.is_empty() {
            candidate.declared.path.clone_from(&native.path);
        }
        candidate.loaded_domains = native.loaded_domains;
        if !native.running_program.is_empty() {
            candidate.observed_command = native.running_program;
        } else if candidate.observed_command.is_empty() {
            candidate.observed_command = native.program;
        }
    }
    if !resident_owner_discovered {
        return Err(DeployError(
            "loaded-unit scan omitted the exact resident transaction owner".to_string(),
        ));
    }
    Ok(candidates.into_values().collect())
}
