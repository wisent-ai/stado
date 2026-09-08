//! Dynamic pricing, cost allocation, forecast, anomalies, and savings ledger.
//!
//! The components are the seams this file already carried: [`prices`] holds
//! the price records and the per-provider reads that fill them, [`allocation`]
//! puts an hourly price on every inventory resource and folds the result into
//! buckets, [`forecast`] projects the burn against the policy budgets and
//! reports the anomalies, and [`ledger`] measures decision outcomes,
//! summarizes savings and persists the reports. Every name a caller outside
//! this module uses is re-exported here, so `crate::autonomy::cost::<item>`
//! resolves exactly as before.

mod allocation;
mod forecast;
mod ledger;
mod prices;

// `super::model` for the moved comparison that names
// `super::model::Ownership::Unknown` verbatim in `forecast`.
use crate::autonomy::model;

pub use allocation::{build_allocation, enrich_inventory};
pub use forecast::{detect_anomalies, forecast};
pub use ledger::outcomes::measure_outcomes;
pub use ledger::reports::{load_billing_snapshot, persist_reports};
pub use ledger::savings::{summarize_savings, summarize_savings_with_measurements};
pub use prices::{refresh_prices, PriceBook, PriceQuote};

const HOURS_PER_DAY: f64 =
    (crate::monitor::billing::SECONDS_PER_DAY / crate::monitor::billing::SECONDS_PER_HOUR) as f64;
const BILLING_MONTH_DAYS: f64 =
    (u64::BITS / (u16::BITS / u8::BITS) - (u16::BITS / u8::BITS)) as f64;
const HOURS_PER_MONTH: f64 = HOURS_PER_DAY * BILLING_MONTH_DAYS;
