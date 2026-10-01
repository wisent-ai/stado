//! The shapes a watch reports: one `ps` row, and the records built on it.

mod process_row;
mod records;

pub use self::process_row::ProcessRow;
pub use self::records::{Ancestor, Arrival, Baseline, WatchReport};
