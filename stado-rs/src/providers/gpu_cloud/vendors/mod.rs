//! One module per GPU cloud vendor: its `PROFILE` (settings, credential
//! fields, offers, guest identity) and its `Api` (the vendor's own calls).
//! Each module cites the vendor documentation its calls follow.

pub mod arkane;
pub mod crusoe;
pub mod cudo;
pub mod hyperstack;
pub mod lambda;
pub mod latitude;
pub mod nebius;
pub mod oblivus;
pub mod oracle;
pub mod runpod;
pub mod salad;
pub mod scaleway;
pub mod voltage_park;
pub mod vultr;

pub(crate) mod signing;
