//! What a host is doing right now and how it is put back in order: whether
//! it answers, how long it has been up, what is cleaned off it, and the one
//! restart the fleet is allowed to ask for.

pub mod cleanup;
pub mod ping;
pub mod reboot;
pub mod uptime;
