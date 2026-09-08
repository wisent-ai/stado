//! What the fleet is made of: the hosts it runs on, the units it runs, the
//! releases it delivers, and the credentials all three authenticate with.

pub(in crate::doctor) mod credentials;
pub(in crate::doctor) mod hosts;
pub(in crate::doctor) mod releases;
pub(in crate::doctor) mod units;
