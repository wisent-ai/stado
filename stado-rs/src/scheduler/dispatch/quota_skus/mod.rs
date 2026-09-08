//! Cross-provider GPU catalog enumerator + write-side fan-outs.
//!
//! Port of `stado/scheduler/dispatch/quota_skus.py`: `provider_catalog`
//! returns the full list of GPU-related SKUs/families the provider
//! supports along with the current per-region limit on file for our
//! project. Backs `stado quota catalog` (read-side enumeration) and
//! `stado quota request-all` (bulk fan-out of CreateQuotaPreference
//! across every enumerated family × every configured region), plus
//! `gcp_request_status` (backs `stado quota requests`).
//!
//! GCP path: the Python code uses google-cloud-quotas `list_quota_infos`;
//! this port calls the Cloud Quotas REST API directly
//! (`GET https://cloudquotas.googleapis.com/v1/projects/{p}/locations/
//! global/services/compute.googleapis.com/quotaInfos`) with gcp_auth,
//! filtered to GPU-related quota_ids:
//!   - NVIDIA-{FAMILY}-GPUS-per-project-region   (legacy per-family
//!     quotas, one per GPU model, dimensioned by region)
//!   - GPUS-PER-GPU-FAMILY-per-project-region    (newer unified quota,
//!     dimensioned by gpu_family + region)
//!
//! The newer GPUS-PER-GPU-FAMILY quota is the right submission target;
//! the legacy per-family quotas are kept for read-side completeness so a
//! catalog dump shows everything Google tracks.
//!
//! Azure path uses `az vm list-skus` to enumerate Compute GPU VM families
//! in the subscription, as a subprocess like Python.

mod azure;
mod catalog;
mod client;
mod gcp;

/// The two write-side fan-outs call back into the sibling dispatch module
/// (`gcp_request_for_family`, `azure_request_increase`, `merge_object`).
/// Binding the sibling here — not re-exporting it — is what keeps their
/// `super::quota_request::` paths resolving from inside this tree.
use super::quota_request;

/// `azure_catalog` is the read arm `provider_catalog` dispatches to and the
/// enumerator `azure_request_all_families` discovers families from;
/// `azure_request_all_families` is named by `crate::cli::quota::submit::
/// increase`, and `azure_rows_from_skus` is the pure SKU-table half its
/// doc comment splits out for tests. All three keep their published
/// `dispatch::quota_skus::` paths.
pub use azure::{azure_catalog, azure_request_all_families, azure_rows_from_skus};
/// `all_catalogs` is named by `crate::cli::quota::read`, whose
/// `stado quota catalog` handler renders the per-provider map;
/// `provider_catalog` is the single-provider arm it wraps.
pub use catalog::{all_catalogs, provider_catalog};
/// `CloudQuotasClient::new` is called by `crate::cli::quota::report`,
/// `crate::cli::quota::submit::increase` and the sibling
/// `crate::scheduler::dispatch::quota_request`; `CatalogError` is the error
/// their signatures carry (`quota_request` lifts it through its own error
/// enum), and `CLOUD_QUOTAS_BASE` is the endpoint that constructor binds.
pub use client::{CatalogError, CloudQuotasClient, CLOUD_QUOTAS_BASE};
/// `gcp_request_status` is named by `crate::cli::quota::report` and
/// `gcp_request_all_families` by `crate::cli::quota::submit::increase`;
/// `gcp_catalog` is the read arm `provider_catalog` dispatches to.
pub use gcp::{gcp_catalog, gcp_request_all_families, gcp_request_status};

/// Named by the sibling `crate::scheduler::dispatch::quota_request`, which
/// resolves the same env-only project before constructing its own client.
pub(super) use gcp::gcp_project_env;
