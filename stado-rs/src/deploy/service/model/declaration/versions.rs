use crate::deploy::service::*;

/// A unit delivered through the compiled managed-product catalog whose host
/// declares no desired version for that product.
///
/// Stado has two independent delivery mechanisms:
///
/// - [`crate::release_control`] owns the desired version of blue-green and
///   replace releases in `release_control.products.<product>.desired`;
/// - [`crate::deploy::products`] owns `host release`, whose per-host desired
///   versions live in `targets[].managed_versions`.
///
/// Requiring both declarations for one product creates two authorities and,
/// for release-control-only products such as Brama, recommends a
/// `host declare-version` command the compiled managed-product catalog refuses.
/// This row therefore exists only for units that map to that compiled catalog
/// and are not owned by release control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredServiceVersion {
    pub host: String,
    /// The exact compiled product name accepted by `host declare-version`.
    pub product: String,
    pub name: String,
    pub unit: String,
    pub program: String,
}

impl UndeclaredServiceVersion {
    pub fn sentence(&self) -> String {
        format!(
            "{} ({}) runs {} from Stado's delivery tree as managed product {:?}, but {} \
             declares no version for it. Declare one with `stado host declare-version {} \
             --binary {} --version X.Y.Z`; that command is valid because {} is present in {}",
            self.name,
            self.unit,
            self.program,
            self.product,
            self.host,
            self.host,
            self.product,
            self.product,
            crate::deploy::products::DECLARATION_PATH,
        )
    }

    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "product": self.product,
            "name": self.name,
            "unit": self.unit,
            "program": self.program,
            "evidence": {"kind": "managed-delivery-unit"},
            "detail": self.sentence(),
        })
    }
}

/// The delivery-tree segment a program path sits under, when it is one.
fn delivery_tree_product(program: &str) -> Option<&str> {
    let (_, tail) = program.split_once("/.stado/services/")?;
    let product = tail.split('/').next()?;
    if product.is_empty() || !tail.contains('/') {
        return None;
    }
    Some(product)
}
/// Whether one registry unit executes Stado from its independently installed
/// service tree rather than from the host-global `$HOME/.stado/bin/stado`.
///
/// The path shape is the same declaration [`delivery_tree_product`] already
/// uses for release inventory. Reusing it keeps release selection and reader
/// convergence from inventing two meanings of "service-local".
pub fn is_service_local_stado_reader(service: &ManagedService) -> bool {
    delivery_tree_product(&service.program).is_some()
        && service.program.rsplit('/').next() == Some("stado")
}

/// Resolve a delivery-tree unit to the exact product name the compiled
/// `host release` catalog accepts.
///
/// Most units are staged under that product name. A few service-specific trees
/// run a catalog binary (for example a control-plane unit staged under its
/// label but executing `stado`), so the program file name is the second
/// supported witness. An arbitrary tree installed by `service update` is not a
/// managed-product declaration and gets no invented semver contract.
fn managed_product_name(delivery_product: &str, program: &str) -> Option<String> {
    let file_name = program.rsplit('/').next().unwrap_or_default();
    let products = crate::deploy::products::declared().ok()?;
    products
        .iter()
        .find(|entry| entry.name == delivery_product || entry.name == file_name)
        .map(|entry| entry.name.clone())
}

/// Every label `stop_legacy` boots out on TARGET, across every product the
/// rollout policy declares for it.
///
/// A unit in this set is scheduled for bootout, not for service liveness or
/// managed-product delivery, so both doctor checks use this one answer.
pub(crate) fn legacy_launchd_labels(
    target: &ComputeTarget,
    control: Option<&crate::release_control::ReleaseControl>,
) -> BTreeSet<String> {
    control
        .into_iter()
        .flat_map(|control| control.products.values())
        .filter_map(|policy| policy.targets.get(&target.name))
        .filter_map(|policy_target| policy_target.legacy_launchd_label.clone())
        .collect()
}

/// Every release-control product targeting this host.
fn release_control_products(
    target: &ComputeTarget,
    control: Option<&crate::release_control::ReleaseControl>,
) -> BTreeSet<String> {
    control
        .into_iter()
        .flat_map(|control| &control.products)
        .filter(|(_, policy)| policy.targets.contains_key(&target.name))
        .map(|(product, _)| product.clone())
        .collect()
}

/// Every compiled managed-product unit on TARGET whose host declares no
/// `managed_versions` entry.
pub fn managed_units_without_declared_version(
    target: &ComputeTarget,
    control: Option<&crate::release_control::ReleaseControl>,
) -> Vec<UndeclaredServiceVersion> {
    let legacy_labels = legacy_launchd_labels(target, control);
    let release_products = release_control_products(target, control);
    let mut rows: BTreeMap<String, UndeclaredServiceVersion> = BTreeMap::new();

    for service in declared_services(target) {
        if legacy_labels.contains(service.unit_id()) {
            continue;
        }
        let Some(delivery_product) = delivery_tree_product(&service.program) else {
            continue;
        };
        if release_products.contains(delivery_product) {
            continue;
        }
        let Some(product) = managed_product_name(delivery_product, &service.program) else {
            continue;
        };
        if target.declared_version(&product).is_some() {
            continue;
        }
        rows.entry(product.clone())
            .or_insert_with(|| UndeclaredServiceVersion {
                host: target.name.clone(),
                product,
                name: service.name.clone(),
                unit: service.unit_id().to_string(),
                program: service.program.clone(),
            });
    }

    rows.into_values().collect()
}
