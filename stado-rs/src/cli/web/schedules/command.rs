//! The command one scheduled request runs as a fleet job on the product's own
//! host.
//!
//! The request goes to the unit's loopback port, not the public hostname: the
//! launcher binds the unit to loopback and the job is pinned to the host the
//! unit runs on, so the call never leaves it and never depends on the edge or
//! DNS. The secret travels as the job's secret environment and reaches curl on
//! its standard input (`--header @-`), so the value is in no argument list a
//! process listing shows. `--fail-with-body` makes an answer of 400 or above
//! fail the job with the body the route answered, which is what
//! `stado job watch` shows.

use std::net::Ipv4Addr;

use crate::config::{WebApiProduct, WebApiSchedule};

/// The variable the job's secret environment delivers the header value in.
pub(super) const SECRET_VARIABLE: &str = "WEB_SCHEDULE_SECRET";

/// The shell command for one declared schedule. The path and method were
/// checked by the declaration parser to need no quoting beyond single quotes.
pub(super) fn request_command(product: &WebApiProduct, schedule: &WebApiSchedule) -> String {
    let url = format!(
        "http://{}:{}{}",
        Ipv4Addr::LOCALHOST,
        product.port(),
        schedule.path()
    );
    let curl = format!(
        "curl --fail-with-body --silent --show-error --request {} '{url}'",
        schedule.method()
    );
    match schedule.secret() {
        None => curl,
        Some(secret) => {
            let prefix = secret
                .scheme()
                .map(|scheme| format!("{scheme} "))
                .unwrap_or_default();
            format!(
                "printf '%s: {prefix}%s\\n' '{}' \"${SECRET_VARIABLE}\" | {curl} --header @-",
                secret.header()
            )
        }
    }
}

/// `ENV=role#field` for the job's secret environment, when the request
/// carries a secret.
pub(super) fn secret_env(schedule: &WebApiSchedule) -> Vec<String> {
    schedule
        .secret()
        .map(|secret| vec![format!("{SECRET_VARIABLE}={}", secret.reference())])
        .unwrap_or_default()
}
