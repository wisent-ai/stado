//! `stado web status` — one verdict per declared web product.
//!
//! Four facts, in the order a hosted product stops working in, and each read
//! from the thing that actually knows it:
//!
//! 1. **What the product declares** — host, port, hostname, unit — out of the
//!    configuration plane, so the report says what is supposed to be true
//!    before it says what is.
//! 2. **The unit's live state** — from the health beacons through
//!    [`crate::deploy::service::list_services`], the same join
//!    `stado service list` and `stado service status` answer from. Beacon-only
//!    by construction: the moment you most need to know what is supposed to be
//!    running on a host is the moment the host has stopped answering, so this
//!    half costs no ssh at all.
//! 3. **Whether the declared port is held by that unit** — from the host,
//!    through [`crate::deploy::service_serving`], because that is the one
//!    question a declaration cannot answer about itself. `service show` says
//!    `runs` whenever the unit FILE exists, and a mac mini spent days with a
//!    dead unit reported healthy while a different launchd job held its port.
//! 4. **What the hostname actually resolves to** — because a unit that serves
//!    perfectly behind a record pointing at a retired edge is an outage, and
//!    it is the one failure every other reader here is blind to.
//!
//! The overall word is the first of those that is wrong, so it names the thing
//! to repair rather than the last symptom. A product that is not `serving`
//! makes the command exit non-zero: a status command that reports a broken
//! product and exits zero is a status command nothing can gate on.
//!
//! The three readers each answer one of those questions and nothing else, so
//! they sit in `readers`; `examine` is the precedence between their answers
//! and owns none of the reading; `report` chooses the products and prints
//! them. The words a verdict can be, the words a lookup can return and the
//! shape one product's answer takes stay here, because every one of those
//! parts names them.

mod examine;
mod readers;
mod report;

use std::time::Duration;

use serde_json::Value;

// `super` inside a component of this module is `status`, not `web`, so the
// three items the moved bodies reach for by that name are bound here and go
// on resolving to exactly the items they named before the split.
use super::{product, unit_label, UNIT_DOMAIN};

pub(crate) use report::status;

/// The unit is loaded, its declared port is held by its own process, and the
/// hostname resolves where this product's edge is.
const VERDICT_SERVING: &str = "serving";
/// Nothing in the registry manages this product's unit yet. `stado web
/// declare` records a product; `stado web deploy` is what makes it run.
const VERDICT_NOT_DEPLOYED: &str = "not-deployed";
/// The unit is managed and the host says it is not running.
const VERDICT_UNIT_DOWN: &str = "unit-down";
/// The unit is loaded and its own declared port is not held by its own
/// process — dead, taken by another job, or unreadable.
const VERDICT_PORT_UNHELD: &str = "port-unheld";
/// The unit serves and the public hostname does not point at this product's
/// edge, so the public name reaches something else or nothing at all.
const VERDICT_DNS_ELSEWHERE: &str = "dns-elsewhere";
/// The selected Stado edge has no valid declaration.
const VERDICT_EDGE_UNCONFIGURED: &str = "edge-unconfigured";

/// The hostname resolved to at least one address.
const DNS_RESOLVED: &str = "resolved";
/// The resolver answered and the name has no address.
const DNS_UNRESOLVED: &str = "unresolved";
/// The resolver did not answer inside the window. Deliberately not folded
/// into [`DNS_UNRESOLVED`]: "this name has no address" and "nobody could ask"
/// are opposite findings, and only one of them is the product's fault.
const DNS_UNREADABLE: &str = "unreadable";

/// How long one hostname lookup may take.
///
/// Bounded because a status read over every declared product must not hang on
/// one unreachable resolver, and short because a name that needs longer than
/// this is already the finding. Two seconds is the window `doctor.rs` puts
/// around its own reachability lookups.
const DNS_TIMEOUT: Duration = Duration::from_secs(2);

/// One product's verdict and the row that explains it.
struct Verdict {
    row: Value,
    word: &'static str,
}
