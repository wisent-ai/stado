//! Job lifecycle: first admission ([`admission`]), the durable transition
//! protocol every move goes through ([`transitions`]), the claim and worker
//! lease operations built on it ([`claims`]), and the queued-document
//! rewrites and moves that share the same recovery ([`mutations`]).

mod admission;
mod claims;
mod mutations;
mod transitions;
