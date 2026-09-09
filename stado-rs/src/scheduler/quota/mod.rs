//! GPU quota tracking — live from each provider's quota API, the storage
//! file is reservation overlay only.
//!
//! Ports both quota reads and the reservation overlay. GCP reads regional
//! limits through the Compute REST API; Azure reads regional
//! Microsoft.Compute usages through ARM and converts family vCPU limits to
//! schedulable GPU slots using the machine catalog.
//!
//! The seams the module already had are the files here: `error` holds the
//! one error every read folds into, `scalar` the Python `int()` coercion
//! both halves share, `live` the two per-provider limit readers, and
//! `compose` the reservation overlay plus the availability count and the
//! cross-provider summary derived from it.

mod compose;
mod error;
mod live;
mod scalar;

pub use compose::available::get_available_instances;
pub use compose::overlay::{load_overlay, load_quotas};
pub use compose::summary::{summarize_quotas, QuotaRow};
pub use error::QuotaError;
pub use live::azure::fetch_quotas_azure;
pub use live::gcp::{fetch_quotas_gcp, GCP_METRIC_TO_ACCEL};
