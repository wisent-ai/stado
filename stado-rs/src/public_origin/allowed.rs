//! Which services may have a public origin at all.
//!
//! A service is reachable from the internet only when the product catalog
//! marks it public on purpose: `service.public.reason` in
//! `catalog/products.yml` (a landing page, the release channel products
//! update from). Every other service — a model gateway, a vault, an object
//! store that is not the release channel — has no public origin, and Stado
//! refuses to declare or converge one for it.
//!
//! An origin forwards to a loopback port on its target, and the service
//! directory is the fleet's statement of which service owns which port on
//! which host, so the origin's service is the directory service whose
//! endpoint on the target has that port. The directory key is matched to the
//! catalog by the product's `directory_service`, else by the product id.

use serde_json::Value;

use crate::targets::Registry;

/// The service a public origin forwards to, and whether the catalog allows
/// it to be public.
#[derive(Debug, Clone, PartialEq)]
pub struct Allowance {
    /// The directory service the upstream port belongs to on the target.
    pub service: Option<String>,
    /// The catalog's reason for publishing it; `None` when it gives none.
    pub reason: Option<String>,
}

impl Allowance {
    pub fn allowed(&self) -> bool {
        self.reason.is_some()
    }

    /// The refusal for an origin that is not allowed, naming the service and
    /// the catalog field.
    pub fn refusal(&self, origin: &str) -> Option<String> {
        if self.allowed() {
            return None;
        }
        Some(match &self.service {
            Some(service) => format!(
                "public origin {origin:?} would publish the service {service}, which the product \
                 catalog does not mark public (catalog/products.yml service.public.reason); our \
                 services have no public entry point unless the catalog states why one does"
            ),
            None => format!(
                "public origin {origin:?} forwards to a port the service directory assigns to no \
                 service on its target, so which service it would publish cannot be told; \
                 nothing is published that the catalog does not mark public"
            ),
        })
    }
}

/// The catalog's reason for publishing `service`, from the product whose
/// `service.directory_service` is it, else the product of that id, read
/// from the catalog compiled into this binary.
fn catalog_reason(service: &str) -> Result<Option<String>, String> {
    let catalog = stado_product::catalog::embedded()
        .map_err(|error| format!("the compiled product catalog is invalid: {error:#}"))?;
    let products = catalog["products"]
        .as_array()
        .ok_or("the compiled product catalog holds no products")?;
    let product = products
        .iter()
        .find(|product| product["service"]["directory_service"].as_str() == Some(service))
        .or_else(|| {
            products
                .iter()
                .find(|product| product["id"].as_str() == Some(service))
        });
    Ok(product
        .and_then(|product| product["service"]["public"]["reason"].as_str())
        .filter(|reason| !reason.trim().is_empty())
        .map(str::to_string))
}

/// Whether the catalog allows the service behind `upstream` on `target` to
/// be public.
pub fn allowance(registry: &Registry, target: &str, upstream: &str) -> Result<Allowance, String> {
    let port = url::Url::parse(upstream)
        .map_err(|error| format!("upstream {upstream} is not a URL: {error}"))?
        .port()
        .ok_or_else(|| format!("upstream {upstream} names no port"))?;
    let service = registry.service_directory.as_ref().and_then(|directory| {
        directory.services.iter().find_map(|(name, service)| {
            service
                .endpoints
                .get(target)
                .and_then(|endpoint| url::Url::parse(&endpoint.url).ok())
                .and_then(|url| url.port())
                .filter(|listening| *listening == port)
                .map(|_| name.clone())
        })
    });
    let reason = match &service {
        Some(service) => catalog_reason(service)?,
        None => None,
    };
    Ok(Allowance { service, reason })
}
