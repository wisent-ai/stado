//! One reader per place a resource lives, each returning an `InventorySource`.
//!
//! [`local`] reads the capacity publications the agents write, [`gcp`] folds
//! the blast-radius probe report into records, [`aws`] walks the EC2 and S3
//! reads page by page, and [`azure`] pages the Resource Graph query. Each
//! reader reports its own coverage, missing permissions and upstream error,
//! so a blocked provider degrades the snapshot instead of failing it.

mod aws;
mod azure;
mod gcp;
mod local;

pub(super) use aws::collect_aws;
pub(super) use azure::collect_azure;
pub(super) use gcp::collect_gcp;
pub(super) use local::collect_local;
