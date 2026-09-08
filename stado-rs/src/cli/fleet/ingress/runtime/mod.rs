//! What the entrance is made of on this machine: the binaries it runs, the
//! loopback port it reserves, and the two process groups it starts and stops.
//!
//! Everything here is local and synchronous, and none of it decides whether the
//! entrance works — that verdict belongs to the sibling `verify` component,
//! which only ever asks the network.

pub(in crate::cli::fleet::ingress) mod binaries;
pub(in crate::cli::fleet::ingress) mod port;
pub(in crate::cli::fleet::ingress) mod process;
