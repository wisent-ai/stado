//! The Cloudflare API calls this command surface makes: one authenticated
//! client, the tunnel access scope it is addressed with, the payload readers
//! every response passes through, and the validators every argument passes
//! before it becomes part of a path or a record name.

mod access;
mod client;
mod payload;
mod validate;

pub(super) use access::{required_field, tunnel_access, TunnelAccess};
pub(super) use payload::{exact_zone_id, required_string, result_array};
pub(super) use validate::{
    belongs_to_zone, validate_api_component, validate_dns_name, validate_origin,
    validate_zone_hostname,
};
