//! One named provider's endpoint, signing region and addressing, built from
//! its profile and what the deployment declared for it.

use super::{Declares, EndpointRule, RegionRule, StorageVendor};

/// Where one provider is reached and how its requests are signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reached {
    pub endpoint: String,
    pub region: String,
    pub path_style: bool,
}

/// The value one placeholder is filled with, refused by the binding's name
/// when the deployment declared none.
fn declared<'a>(value: &'a str, what: &str, vendor: StorageVendor) -> Result<&'a str, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!(
            "the {} storage backend ({}) needs storage.s3_compatible.{what}; nothing declares it",
            vendor.id(),
            vendor.profile().product
        ));
    }
    if value.contains(['/', '?', '#', '@', ' ']) {
        return Err(format!(
            "storage.s3_compatible.{what} {value:?} is not a single {} name",
            vendor.profile().product
        ));
    }
    Ok(value)
}

/// Resolve `vendor`'s endpoint from the declared `endpoint`, `account` and
/// `region`, each read only where the provider's profile asks for it.
pub fn resolve(
    vendor: StorageVendor,
    endpoint: &str,
    account: &str,
    region: &str,
) -> Result<Reached, String> {
    let profile = vendor.profile();
    let (needs_account, needs_region) = match profile.declares {
        Declares::Account => (true, false),
        Declares::Region => (false, true),
        Declares::AccountAndRegion => (true, true),
        Declares::Nothing => (false, false),
        Declares::EndpointAndRegion => (false, true),
    };
    let region = match profile.region {
        RegionRule::Fixed(fixed) => fixed.to_string(),
        RegionRule::Declared => declared(region, "region", vendor)?.to_string(),
    };
    let endpoint = match profile.endpoint {
        EndpointRule::Template(template) => {
            let mut built = template.to_string();
            if needs_account {
                built = built.replace("{account}", declared(account, "account", vendor)?);
            }
            if needs_region {
                built = built.replace("{region}", &region);
            }
            built
        }
        EndpointRule::Declared => {
            let declared = endpoint.trim();
            if declared.is_empty() {
                return Err(format!(
                    "the {} storage backend ({}) runs where the deployment says: \
                     storage.s3_compatible.endpoint must name its URL",
                    vendor.id(),
                    profile.product
                ));
            }
            declared.to_string()
        }
    };
    Ok(Reached {
        endpoint,
        region,
        path_style: profile.path_style,
    })
}
