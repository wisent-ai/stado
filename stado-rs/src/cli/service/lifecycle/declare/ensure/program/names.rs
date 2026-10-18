//! Which init-system identity a service's unit carries.

use crate::cli::CmdError;
use crate::deploy::service::ManagedService;

/// The suffix systemd gives a unit's name; launchd names the same unit
/// without it.
const SYSTEMD_SUFFIX: &str = ".service";

/// `label` as launchd names it: without the suffix systemd appends.
fn launchd_name(label: &str) -> &str {
    match label.strip_suffix(SYSTEMD_SUFFIX) {
        Some(bare) => bare,
        None => label,
    }
}

/// The label a managed service's declaration names, or none when it names
/// an empty one.
pub(crate) fn declared_label(service: &ManagedService) -> Option<&str> {
    Some(launchd_name(service.unit_id())).filter(|label| !label.is_empty())
}

/// The stable init-system identity of `name` on `target`, from the service
/// catalog's `entry` (authored services) or the managed-product catalog
/// (units that execute a delivered product binary). Two declarations naming
/// different units for one service are refused as configuration.
pub(in crate::cli::service::lifecycle::declare::ensure) fn canonical_unit(
    name: &str,
    target: &crate::targets::ComputeTarget,
    entry: Option<&crate::deploy::service_catalog::CatalogService>,
) -> Result<Option<String>, CmdError> {
    let catalog_unit = entry.and_then(|entry| entry.unit.clone());
    let managed_unit = canonical_managed_unit(name, &target.name)?;
    match (catalog_unit, managed_unit) {
        (Some(service_unit), Some(product_unit)) if service_unit != product_unit => {
            Err(CmdError::click(format!(
                "service and managed-product declarations disagree about {name}: {service_unit} \
                 versus {product_unit}"
            ))
            .stating(crate::primitives::failure::FailureCode::Config))
        }
        (Some(unit), _) | (_, Some(unit)) => Ok(Some(unit)),
        (None, None) => Ok(None),
    }
}

/// Stable unit identity supplied by the managed-product declaration.
///
/// Service names are operator-facing leaves (`stado-resolver`), while the
/// product catalog carries the init system's full identity
/// (`com.wisent.stado-resolver`). A registry record may spell the unit
/// differently, so it cannot be the authority for this lookup.
fn canonical_managed_unit(name: &str, target: &str) -> Result<Option<String>, CmdError> {
    let requested = launchd_name(name);
    let mut matched: Option<String> = None;
    let products = crate::deploy::products::declared().map_err(CmdError::from)?;
    for product in products {
        for unit in &product.units {
            let label = unit.label_for(target);
            let bare = launchd_name(&label).to_string();
            // The last dotted word of the identity: `stado-resolver` of
            // `com.wisent.stado-resolver`.
            let leaf = match bare.rsplit_once('.') {
                Some((_, leaf)) => leaf,
                None => bare.as_str(),
            };
            if label != name && bare != requested && leaf != requested {
                continue;
            }
            // A product declares a unit once per init system: `com.wisent.stado`
            // for launchd and `com.wisent.stado.service` for systemd are one
            // unit, so they are compared without the systemd suffix.
            if matched.as_deref().is_some_and(|existing| existing != bare) {
                return Err(CmdError::click(format!(
                    "managed product declarations give {name} more than one unit identity"
                ))
                .stating(crate::primitives::failure::FailureCode::Config));
            }
            matched = Some(bare);
        }
    }
    Ok(matched)
}
