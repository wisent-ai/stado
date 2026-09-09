//! `stado service serving` against real listening sockets and a real launchd
//! label.
//!
//! Every case drives the built `stado` binary against this machine: the
//! registry target's `hostnames` name this host, the unit file is a
//! LaunchAgent under a tempdir `HOME`, and whatever a case calls a served
//! endpoint is a port a `TcpListener` in this test process really bound.
//!
//! What is defended: the defect that started this — a unit whose port is held
//! by a process its label does not own is never reported as serving; a port
//! nothing listens on is `not_serving` and says which port; a holder whose
//! owning label cannot be read is `unknown` and never `serving`; the command
//! exits non-zero on anything but `serving`; it refuses when no port was
//! named instead of inventing an empty pass; and the declared port is found
//! even when the directory and the host spell the service differently, which
//! is the shape every real placement-backed service has.
//!
//! - [`fixture`]: the fleet, the unit file, and the real loopback listener.
//! - [`ports`]: the verdict about a port and who holds it.
//! - [`declared`]: which port is judged when the operator names none.

mod declared;
mod fixture;
mod ports;
