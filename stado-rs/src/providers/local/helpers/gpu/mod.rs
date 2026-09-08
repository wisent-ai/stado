//! What this host's accelerators are, what they can take, and who else is on
//! them.
//!
//! [`inventory`] reads the boards themselves; [`capacity`] turns those numbers
//! into the accelerator tiers the fleet routes on; [`vast_renter`] answers the
//! one question the driver cannot — whether a paying Vast.ai renter already
//! owns this machine's GPU time.

pub mod capacity;
pub mod inventory;
pub mod vast_renter;
