//! Keeping a host's disk usable: the gate that refuses work when there is no
//! room, the sweep that retires scratch, and the flush that clears fleet
//! state a host no longer owns.

pub mod fleet_flush;
pub mod gate;
pub mod scratch_sweep;
