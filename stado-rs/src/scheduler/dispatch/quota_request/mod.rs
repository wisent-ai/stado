//! Cloud Quotas API quota-increase orchestrator.
//!
//! Port of `stado/scheduler/dispatch/quota_request.py`. Wraps the GCP
//! Cloud Quotas CreateQuotaPreference/UpdateQuotaPreference REST API and
//! Azure Microsoft.Quota create_or_update (ARM REST PUT) so a single
//! `stado quota request <accel> --to N` invocation fans out one
//! quota-increase request per (provider, region) across every provider in
//! WC_PROVIDERS. Co-located in scheduler/dispatch/ because submitting a
//! quota preference is the write-side mirror of dispatch's read-side
//! `get_available_instances`: both treat per-(provider, region, accelerator)
//! GPU ceilings as the unit of work.
//!
//! GCP: the newer Cloud Quotas API expresses GPU quotas as a single
//! quota_id `GPUS-PER-GPU-FAMILY-per-project-region` parameterized by a
//! dimensions={"region": ..., "gpu_family": ...} map. Submission is
//! non-blocking: the QuotaPreference is created or updated and Google's
//! reviewer approves/declines asynchronously. ALREADY_EXISTS is converted
//! to UpdateQuotaPreference so re-running the command bumps an existing
//! pending request to the new preferred_value rather than erroring.
//!
//! Azure: Microsoft.Quota provider, create_or_update against
//! `subscriptions/{sub}/providers/Microsoft.Compute/locations/{loc}`
//! with resource_name = the SKU family name.
//!
//! Deviation: Python's Azure path treats a missing azure-mgmt-quota SDK as
//! an informational {"available": false, "reason": "azure-mgmt-quota not
//! installed"} row. The Rust port calls ARM REST directly (no optional
//! SDK), so only the `AZURE_SUBSCRIPTION_ID unset` unavailable case
//! remains.

mod azure;
mod error;
mod fanout;
mod gcp;

/// The GCP arm and both fan-outs call into the sibling catalog module
/// (`CloudQuotasClient`, `CatalogError`, `gcp_project_env`). Binding the
/// sibling here — not re-exporting it — is what keeps their
/// `super::quota_skus::` paths resolving from inside this tree.
use super::quota_skus;

/// `AZURE_QUOTA_API_VERSION` pins the api-version every Microsoft.Quota
/// URL carries, and `azure_quota_body` / `azure_quota_scope` are the
/// request-shaping pieces the submission's doc comment splits out; all
/// keep their published `dispatch::quota_request::` paths.
pub use azure::{azure_quota_body, azure_quota_scope, AZURE_QUOTA_API_VERSION};
/// `azure_request_increase` is named by the sibling
/// `crate::scheduler::dispatch::quota_skus::azure` bulk fan-out;
/// `azure_request_increase_with_client` is the injectable-client half its
/// doc comment splits out.
pub use azure::{azure_request_increase, azure_request_increase_with_client};
/// `QuotaRequestError` is the error `gcp_request_increase` returns, so it
/// stays nameable wherever that re-exported signature is.
pub use error::QuotaRequestError;
/// `request_quota_increases` is named by `crate::cli::quota::submit::
/// increase`, whose `stado quota request` handler renders the per-target
/// rows; `gcp_fanout` and `azure_fanout` are the per-provider arms it
/// dispatches to.
pub use fanout::{azure_fanout, gcp_fanout, request_quota_increases};
/// `gcp_request_for_family` is named by the sibling
/// `crate::scheduler::dispatch::quota_skus::gcp` bulk fan-out;
/// `gcp_request_increase` is the accel-label wrapper over it, and
/// `GCP_ACCEL_TO_GPU_FAMILY` / `GCP_GPU_FAMILY_QUOTA_ID` /
/// `gcp_preference_id` / `gcp_preference_body` are the request-shaping
/// pieces its doc comment splits out.
pub use gcp::{
    gcp_preference_body, gcp_preference_id, gcp_request_for_family, gcp_request_increase,
    GCP_ACCEL_TO_GPU_FAMILY, GCP_GPU_FAMILY_QUOTA_ID,
};

/// Named by the sibling `crate::scheduler::dispatch::quota_skus`, whose
/// two bulk fan-outs merge their per-target rows the same way.
pub(crate) use fanout::merge_object;
