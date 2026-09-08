//! `stado resources show` — one read-only, provider-neutral inventory.
//!
//! Every source is fault-isolated. A cloud or credential failure is represented
//! as a degraded source instead of erasing the resources returned by the other
//! providers.
//!
//! `model` holds the report vocabulary every component names, `sources` holds
//! one inspector per fault-isolated source, `command` joins them into a single
//! report, and `human` prints the operator-facing tables.

mod command;
mod human;
mod model;
mod sources;

pub use command::run;
