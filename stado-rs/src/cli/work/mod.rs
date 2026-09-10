//! Commands over the work itself: what is queued, what is scheduled, what is
//! cancelled, and how much of it the fleet runs on its own.

pub mod autonomy;
pub mod cancel;
pub mod queue;
pub mod schedule;
