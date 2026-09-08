//! The finished-job row: its shape, the two collectors that build it out of
//! JobStorage, and the wall-time medians derived from a batch of them.

pub(super) mod collect;
pub(super) mod row;
pub(super) mod wall_time;
