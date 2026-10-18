//! Counting: the one place Stado adds a single item to a count, so the step
//! of every count is stated once instead of being spelled at each site.

use std::sync::atomic::{AtomicU64, Ordering};

/// The step of a count: each item adds one, as `Iterator::count` counts
/// (<https://doc.rust-lang.org/std/iter/trait.Iterator.html#method.count>).
const STEP: u8 = 1;

/// A count that one more item joins.
pub trait Tally {
    /// Add one item. A count at its type's maximum stays there rather than
    /// wrapping to a count of fewer items than it saw.
    fn tally(&mut self);
}

macro_rules! tally {
    ($($kind:ty),*) => {$(
        impl Tally for $kind {
            fn tally(&mut self) {
                *self = self.saturating_add(<$kind>::from(STEP));
            }
        }
    )*};
}

tally!(u32, u64, usize, i64);

/// The next number a shared counter hands out, counting this draw: the first
/// draw answers the counter's starting value.
pub fn draw(counter: &AtomicU64) -> u64 {
    counter.fetch_add(u64::from(STEP), Ordering::SeqCst)
}

/// The first number a shared counter hands out when zero is kept to mean
/// "none drawn".
pub const FIRST_DRAW: u64 = STEP as u64;
