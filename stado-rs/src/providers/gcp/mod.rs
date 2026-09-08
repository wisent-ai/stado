//! GCP provider: GCE instance lifecycle.
//!
//! Port of `stado/providers/gcp/__init__.py`. The Python module uses the
//! google-cloud-compute SDK (`compute_v1.InstancesClient`, gRPC); this port
//! talks to the GCE REST API v1 (`https://compute.googleapis.com/compute/v1`)
//! with gcp_auth (cloud-platform scope), the same auth pattern as
//! [`crate::queue::gcs`]. Long-running insert operations are polled via
//! `GET .../zones/{zone}/operations/{op}` until DONE — the Rust equivalent
//! of the Python SDK's `op.result()`.
//!
//! Cross-instance stockout/quota caches live in [`stockout`].
//!
//! Deviation: Python's `GCPProvider()` constructor eagerly builds the SDK
//! client (failing on missing ADC at `get_provider` time). Here
//! [`GcpProvider::from_env`] is lazy — credentials and the JobStorage are
//! resolved on the first API call, so `get_provider("gcp")` stays a cheap,
//! sync factory. Failures surface on the first method call instead.
//!
//! The REST transport lives in `client`, the provider itself in
//! `provider`; both are re-exported here, so every caller keeps naming
//! `crate::providers::gcp::<item>` unchanged.

pub mod inventory;
pub mod stockout;

mod client;
mod provider;

pub use client::{GceClient, GceError, COMPUTE_API_BASE};
pub use provider::{instance_body, region_of_zone, GcpProvider};
