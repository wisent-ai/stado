//! What the product can say about a host that stopped answering.
//!
//! The incident this area exists for: between 18:29 and 18:35 UTC on
//! 2026-08-19 a production host answered no ping and no ssh, then came back.
//! Six minutes of a host being unreachable left no trace anywhere in the
//! product — the beacon prefix holds only the LATEST document per host, so the
//! gap closed over itself the moment the host returned, and the two readers
//! that did notice wrote their refusals to `~/.stado/logs/stado-resolver.err`
//! and nowhere a person would look.
//!
//! Every case here drives the built binary through the surface an operator
//! reaches for during exactly that outage — `stado host link TARGET`, whose
//! own help promises "the silences recorded against it, and what readers
//! refused because of them" — and the silence each case asserts is produced
//! by a real absence on this machine: a beacon object that is not there, a
//! beacon whose own instant is already past the threshold, or an authority
//! whose declared name does not resolve. The host under test is this one,
//! declared in the registry under its own kernel host name.
//!
//! - [`fixture`]: the tempdir fleet and the built binary.
//! - [`report`]: readers of the document the command printed.
//! - [`absence`]: the gap a real absence opens, and what closes it.
//! - [`history`]: what the report carries of a host's earlier gaps.
//! - [`refusals`]: what refused while the host said nothing.

mod absence;
mod fixture;
mod history;
mod refusals;
mod report;
