use super::*;

impl Registry {
    /// Return the named target, or None if not in the registry.
    pub fn lookup(&self, name: &str) -> Option<&ComputeTarget> {
        self.targets.iter().find(|t| t.name == name)
    }

    /// Subset of targets with kind='local'. Used by wc bootstrap.
    pub fn local_targets(&self) -> Vec<&ComputeTarget> {
        self.targets
            .iter()
            .filter(|target| target.is_provider(crate::capabilities::ProviderId::Local))
            .collect()
    }
    /// Return the unique local target selected by a validated placement
    /// heuristic.
    pub fn lookup_host_heuristic(&self, heuristic: &str) -> Option<&ComputeTarget> {
        self.targets
            .iter()
            .find(|target| target.host_heuristic.as_deref() == Some(heuristic))
    }

    /// Return the named coordinator entry.
    pub fn lookup_coordinator(&self, name: &str) -> Option<&Coordinator> {
        self.coordinators.iter().find(|c| c.name == name)
    }
    /// Resolve an operator selector as an exact coordinator name first, then
    /// as its declarative host placement.
    pub fn lookup_coordinator_selector(&self, selector: &str) -> Option<&Coordinator> {
        self.lookup_coordinator(selector).or_else(|| {
            self.coordinators
                .iter()
                .find(|coordinator| coordinator.host_heuristic.as_deref() == Some(selector))
        })
    }

    /// The directory entry for a service, when the registry carries one.
    pub fn service(&self, name: &str) -> Option<&Service> {
        self.service_directory.as_ref()?.services.get(name)
    }

    /// The named placement profile.
    pub fn placement_profile(&self, name: &str) -> Option<&PlacementProfile> {
        self.placement_profiles
            .iter()
            .find(|profile| profile.name == name)
    }

    /// The launchd/systemd label that serves directory service `name` on
    /// `host`, when the registry says which one does.
    ///
    /// The directory keys a service by its logical name -- `brama` -- and a
    /// host keys the same thing by the label launchd knows,
    /// `com.wisent.always-on.brama`. Both names are declared, in two places,
    /// and nothing joined them: every caller that had one and needed the other
    /// re-derived it, or asked an operator for `--port`. This is that join,
    /// once, from the declarations that already carry it --
    /// [`Service::managed_service`] where a service owns its unit outright,
    /// and the placement profile's own `units` map where a profile moves it.
    ///
    /// `None` means the registry does not say, which is not the same as the
    /// service having no unit: a caller must report that it could not tell
    /// rather than assume the label equals the service name.
    pub fn service_unit(&self, name: &str, host: &str) -> Option<&str> {
        let service = self.service(name)?;
        if let Some(unit) = service
            .managed_service
            .as_deref()
            .filter(|unit| !unit.is_empty())
        {
            return Some(unit);
        }
        let profile = self.placement_profile(service.placement_profile.as_deref()?)?;
        let unit = profile.hosts.get(host)?.units.get(name)?;
        if unit.release_controlled() {
            return None;
        }
        [unit.unit.as_str(), unit.name.as_str()]
            .into_iter()
            .find(|label| !label.is_empty())
    }

    /// The directory service that `label` serves on `host`, by the join in
    /// [`Registry::service_unit`].
    ///
    /// The inverse direction, for callers holding the launchd label and
    /// needing the declaration -- which is the direction `service serving` was
    /// missing when it refused a unit label with "the service directory
    /// declares no endpoint".
    pub fn service_named_by_unit(&self, label: &str, host: &str) -> Option<&str> {
        self.service_directory
            .as_ref()?
            .services
            .keys()
            .map(String::as_str)
            .find(|name| self.service_unit(name, host) == Some(label))
    }

    /// The document as JSON, including every top-level key this build does
    /// not model.
    ///
    /// This is what a writer must serialize: unmodelled top-level keys come
    /// back verbatim out of [`Registry::extra`], so a read-modify-write
    /// through this method keeps the blocks a newer publisher added. The one
    /// thing it does not reproduce is what the loader deliberately drops —
    /// target and coordinator entries with no `name` (Python parity) — and
    /// `targets` / `coordinators` are always emitted, empty if the document
    /// carried none.
    pub fn to_document(&self) -> Value {
        serde_json::to_value(self).expect("registry serialization is infallible")
    }

    /// Find the unique target declaring the normalized host identity.
    ///
    /// Names, explicit hostname aliases, and the host part of every declared
    /// SSH connection path are identities. Ambiguous registry data is rejected
    /// rather than allowing target order to decide which configuration a
    /// host receives.
    pub fn lookup_self(&self, hostname: &str) -> Result<Option<&ComputeTarget>, RegistryError> {
        let identity = normalize_hostname(hostname);
        if identity.is_empty() {
            return Ok(None);
        }
        let mut matches: Vec<&ComputeTarget> = Vec::new();
        for target in &self.targets {
            let mut identities: HashSet<String> = HashSet::new();
            identities.insert(normalize_hostname(&target.name));
            identities.extend(
                target
                    .hostnames
                    .iter()
                    .map(|alias| normalize_hostname(alias)),
            );
            identities.extend(
                target
                    .ssh_connections()
                    .map(|(_, destination)| ssh_hostname(destination)),
            );
            if identities.contains(&identity) {
                matches.push(target);
            }
        }
        if matches.len() > 1 {
            let mut names: Vec<&str> = matches.iter().map(|t| t.name.as_str()).collect();
            names.sort_unstable();
            return Err(RegistryError::AmbiguousIdentity {
                identity,
                names: names.join(", "),
            });
        }
        Ok(matches.into_iter().next())
    }
}
