//! What every `quota` subcommand shares: the JSON echo and the `str[:n]`
//! truncation the tables print through, the provider and region CSV flag
//! parsers, the quota-adapter lookup that decides which provider branch a
//! row takes, the contact-email resolution and the GCP project binding the
//! write side targets.

use serde_json::Value;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::providers::azure::AzureError;
use crate::providers::gcp::GceError;
use crate::scheduler::dispatch::quota_replies::RepliesError;
use crate::scheduler::dispatch::quota_skus::CatalogError;
use crate::scheduler::quota::QuotaError;

impl From<CatalogError> for CmdError {
    fn from(error: CatalogError) -> Self {
        let code = error.failure_code();
        Self::click(error.to_string()).stating(code)
    }
}

impl From<RepliesError> for CmdError {
    fn from(error: RepliesError) -> Self {
        let code = error.failure_code();
        Self::click(error.to_string()).stating(code)
    }
}

/// A live quota read states the class of the provider call that failed:
/// missing cloud credentials are authentication, an unreachable cloud API its
/// outage, a corrupt `config/quotas.json` the operator's configuration, and
/// storage and provider failures keep their own class. A cloud API refusal
/// carries no status here and stays unknown rather than guessed.
impl From<QuotaError> for CmdError {
    fn from(error: QuotaError) -> Self {
        let message = error.to_string();
        let code = match error {
            QuotaError::Storage(storage) => return Self::from(storage),
            QuotaError::Provider(provider) => return Self::from(provider),
            QuotaError::Json(_) => FailureCode::Config,
            QuotaError::Gcp(GceError::Auth(_)) | QuotaError::Azure(AzureError::Auth(_)) => {
                FailureCode::Auth
            }
            QuotaError::Gcp(GceError::Http(http)) | QuotaError::Azure(AzureError::Http(http)) => {
                if http.is_timeout() {
                    FailureCode::Timeout
                } else {
                    FailureCode::InfraDown
                }
            }
            QuotaError::Gcp(GceError::Api(_)) | QuotaError::Azure(AzureError::Api(_)) => {
                FailureCode::Unknown
            }
        };
        Self::click(message).stating(code)
    }
}

/// Python `click.echo(json.dumps(payload, indent=2, sort_keys=True))`.
pub(super) fn echo_json(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("Value serialization is infallible");
    println!("{}", crate::models::ensure_ascii(&pretty));
}

/// Python's CSV-flag parse (`[p.strip() for p in arg.split(",") if
/// p.strip()] or WC_PROVIDERS`).
pub(super) fn parse_providers(arg: &str) -> Result<Vec<String>, CmdError> {
    let parsed = arg
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let explicit = !parsed.is_empty();
    let names = if explicit {
        parsed
    } else {
        crate::config::wc_providers().to_vec()
    };
    let selectable = |variant: &crate::capabilities::CapabilityVariant| {
        variant
            .provider
            .is_some_and(|provider| provider.as_str() == variant.id)
    };
    let mut providers = Vec::new();
    for name in names {
        let variant = crate::capabilities::variant(crate::capabilities::RuntimeFacet::Quota, &name)
            .filter(|variant| selectable(variant));
        match variant {
            Some(variant)
                if !providers
                    .iter()
                    .any(|provider: &String| provider.as_str() == variant.id) =>
            {
                providers.push(variant.id.to_string());
            }
            Some(_) | None if !explicit => {}
            None => {
                let choices = crate::capabilities::get("quota")
                    .into_iter()
                    .flat_map(|capability| capability.variants)
                    .filter(|variant| selectable(variant))
                    .map(|variant| variant.id)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(CmdError::usage(format!(
                    "provider {name:?} has no quota adapter; use one of: {choices}"
                )));
            }
            Some(_) => {}
        }
    }
    Ok(providers)
}

pub(super) fn quota_adapter(name: &str) -> Option<crate::capabilities::QuotaAdapter> {
    match crate::capabilities::variant(crate::capabilities::RuntimeFacet::Quota, name)
        .map(|variant| variant.adapter)
    {
        Some(crate::capabilities::RuntimeAdapter::Quota(adapter)) => Some(adapter),
        _ => None,
    }
}

/// Python's regions CSV parse (`... or None`).
pub(super) fn parse_regions(arg: &str) -> Option<Vec<String>> {
    let parsed: Vec<String> = arg
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    }
}

/// `--email` or $WC_QUOTA_CONTACT_EMAIL; "" when neither is set.
pub(super) fn contact_email(flag: &str) -> String {
    if !flag.is_empty() {
        return flag.to_string();
    }
    std::env::var("WC_QUOTA_CONTACT_EMAIL")
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The GCP project the write side targets (env-only, like the Python).
pub(super) fn gcp_project_env() -> String {
    let env = crate::capabilities::config_env(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "project",
    )
    .expect("GCP project binding is missing from the capability catalog");
    std::env::var(env).unwrap_or_else(|_| "wisent-480400".to_string())
}
