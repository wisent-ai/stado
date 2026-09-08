//! Everything about a record that needs a `JobStorage`.
//!
//! `reads` walks a host's prefix newest-key-first and parses what it finds,
//! `observe` holds the single open/close entry point every reader shares,
//! and `refusals` publishes one reader refusal best effort. The pure joins
//! those three stand on are the parent module's `transitions`.

mod observe;
mod reads;
mod refusals;

pub use observe::{observe_beacon_age, observe_beacon_age_at};
pub use reads::{
    open_silence, recent_refusals, recent_refusals_at, recent_silences, refusal_summary,
    refusal_summary_at,
};
pub use refusals::{record_refusal, report_refusal, report_refusal_detached};
