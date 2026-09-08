//! AWS provider: EC2 instance lifecycle.
//!
//! Port of `stado/providers/aws.py`. Python uses boto3; this port uses
//! aws-sdk-ec2 + aws-config. The coordinator reads `stado-aws` through its
//! scoped Skarbiec grant; adapter hosts without a Skarbiec grant use their
//! EC2 IMDSv2 workload identity. Environment credential chains are disabled.
//!
//! Like [`super::gcp::GcpProvider`], the SDK client is resolved lazily on
//! the first API call so `get_provider("aws")` stays a cheap, sync
//! factory.
//!
//! Deviation: the instance_ref is the raw EC2 instance id (Python
//! returns `iid` and `delete_instance`/`instance_exists` pass it back to
//! EC2 verbatim), not the `"name@zone"` shape gcp/azure use.
//!
//! The EC2 contract the provider codes against — the `Ec2Api` trait and the
//! `RunInstanceArgs` bundle one RunInstances attempt takes — lives in
//! `api`; the aws-sdk-ec2 transport and the credential chain behind it in
//! `client`; the provider itself in `provider`; the `[aws]` progress line
//! and the SDK-error lift that carries the EC2 error code in
//! `diagnostics`. [`AwsProvider`] and [`sdk_config`] are re-exported here,
//! so every caller keeps naming `crate::providers::aws::<item>` unchanged.

mod api;
mod client;
mod diagnostics;
mod provider;

pub use provider::AwsProvider;

pub(crate) use client::sdk_config;
