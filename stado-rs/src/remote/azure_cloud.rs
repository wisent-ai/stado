//! The Azure cloud a deployment rents from: the endpoints every Azure call
//! reaches, read once from the deployment's compute bindings.
//!
//! Azure runs as several clouds with their own Resource Manager and token
//! authority (the public cloud, Azure US Government, Azure China 21Vianet),
//! so which one a deployment rents from is its to declare, not a constant.
//! Undeclared, each endpoint is the public cloud's. A declared endpoint must
//! be HTTPS, except one on a loopback address, which is where a listener
//! standing in for Azure answers, or on a link-local one, where the instance
//! metadata service answers plain HTTP on the machine itself; anything else
//! is refused naming the binding, before any request is sent.
//!
//! - `azure.cloud.resource_manager` (`AZURE_RESOURCE_MANAGER`): the ARM base
//!   every resource path is appended to, and the audience tokens are asked for;
//! - `azure.cloud.authority` (`AZURE_AUTHORITY`): the OAuth authority sign-in
//!   and client-credentials requests go to;
//! - `azure.cloud.identity_endpoint` (`AZURE_IDENTITY_ENDPOINT`): the instance
//!   metadata token endpoint a managed identity answers on;
//! - `azure.cloud.retail_prices` (`AZURE_RETAIL_PRICES`): the Retail Prices
//!   API machine prices are read from, which the credit gate and the price
//!   book quote.

use std::sync::LazyLock;

/// The public cloud's Resource Manager.
const PUBLIC_RESOURCE_MANAGER: &str = "https://management.azure.com";
/// The public cloud's OAuth authority.
const PUBLIC_AUTHORITY: &str = "https://login.microsoftonline.com";
/// The instance metadata service's managed-identity token endpoint: the same
/// link-local address in every Azure cloud, answering plain HTTP.
const PUBLIC_IDENTITY_ENDPOINT: &str = "http://169.254.169.254/metadata/identity/oauth2/token";
/// The public cloud's Retail Prices API.
const PUBLIC_RETAIL_PRICES: &str = "https://prices.azure.com/api/retail/prices";

/// The endpoints one Azure cloud answers on, each without a trailing slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cloud {
    /// The ARM base resource paths are appended to, and the token audience.
    pub resource_manager: String,
    /// The OAuth authority; a tenant's endpoints are below it.
    pub authority: String,
    /// The managed-identity token endpoint.
    pub identity_endpoint: String,
    /// The Retail Prices API.
    pub retail_prices: String,
}

impl Cloud {
    /// The OAuth scope of a token for this cloud's Resource Manager.
    pub fn scope(&self) -> String {
        format!("{}/.default", self.resource_manager)
    }

    /// `<authority>/<tenant>/oauth2/v2.0/<endpoint>`: a tenant's OAuth
    /// endpoint (`token`, `authorize`) in this cloud.
    pub fn oauth(&self, tenant: &str, endpoint: &str) -> String {
        format!(
            "{}/{}/oauth2/v2.0/{endpoint}",
            self.authority,
            tenant.trim()
        )
    }
}

static DECLARED: LazyLock<Result<Cloud, String>> = LazyLock::new(|| {
    Ok(Cloud {
        resource_manager: endpoint(
            crate::config::azure_resource_manager(),
            PUBLIC_RESOURCE_MANAGER,
            "azure.cloud.resource_manager (environment override AZURE_RESOURCE_MANAGER)",
        )?,
        authority: endpoint(
            crate::config::azure_authority(),
            PUBLIC_AUTHORITY,
            "azure.cloud.authority (environment override AZURE_AUTHORITY)",
        )?,
        identity_endpoint: endpoint(
            crate::config::azure_identity_endpoint(),
            PUBLIC_IDENTITY_ENDPOINT,
            "azure.cloud.identity_endpoint (environment override AZURE_IDENTITY_ENDPOINT)",
        )?,
        retail_prices: endpoint(
            crate::config::azure_retail_prices(),
            PUBLIC_RETAIL_PRICES,
            "azure.cloud.retail_prices (environment override AZURE_RETAIL_PRICES)",
        )?,
    })
});

/// The cloud this deployment declares, read once per process; a declared
/// endpoint that is neither HTTPS nor on a loopback or link-local address is
/// the refusal, naming its binding.
pub fn declared() -> Result<&'static Cloud, String> {
    DECLARED.as_ref().map_err(Clone::clone)
}

/// `declared` as an endpoint, or `public` when it is empty: an HTTPS URL, or
/// one on a loopback or link-local address; its trailing slash dropped.
fn endpoint(declared: &str, public: &str, binding: &str) -> Result<String, String> {
    let declared = declared.trim();
    if declared.is_empty() {
        return Ok(public.to_string());
    }
    let rule = "must be an HTTPS URL, or one on a loopback or link-local address";
    let parsed = url::Url::parse(declared)
        .map_err(|error| format!("{binding} {rule}; {declared:?} is not a URL: {error}"))?;
    let on_machine = match parsed.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback() || address.is_link_local(),
        Some(url::Host::Ipv6(address)) => address.is_loopback() || address.is_unicast_link_local(),
        Some(url::Host::Domain(name)) => name == "localhost",
        None => false,
    };
    if parsed.host().is_none() || (parsed.scheme() != "https" && !on_machine) {
        return Err(format!("{binding} {rule}; {declared:?} is neither"));
    }
    Ok(declared.trim_end_matches('/').to_string())
}
