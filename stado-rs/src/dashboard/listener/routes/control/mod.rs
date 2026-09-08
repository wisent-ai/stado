//! The control plane this listener serves beside the object data plane: the
//! declared hosts ([`host`]), the canonical machine requests ([`machine`]),
//! the shared rate limit ([`rate_limit`]), and the managed services
//! ([`service`]).

mod host;
mod machine;
mod rate_limit;
mod service;

pub(crate) use host::host_inventory_target;
pub(crate) use machine::machine_result_response;
pub(crate) use service::service_name;
