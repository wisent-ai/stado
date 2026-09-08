//! What Compute Engine holds: the instances, the disks behind them, the
//! regional quota that caps them, and the addresses reserved for them.

pub(super) mod addresses;
pub(super) mod disks;
pub(super) mod instances;
pub(super) mod quotas;
