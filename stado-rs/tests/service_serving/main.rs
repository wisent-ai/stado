//! `stado service serving` against real listening sockets and a real launchd
//! label.
//!
//! Every test drives the built `stado` binary. The registry target's
//! `hostnames` name THIS machine, so `deploy/host_channel.rs` runs the remote
//! script locally through the same `/bin/bash -s` the ssh branch asks the login
//! shell for — the script under test is byte-identical either way, and only the
//! hop disappears. HOME is a tempdir, so the unit file being read is real state
//! this test made.
//!
//! There is no stub socket table and no fake process tree. The "listener" is a
//! `TcpListener` this test binds on loopback, and the pid the command reports
//! as holding it is this test process. The owner walk therefore runs against
//! this machine's real `launchctl list` and this test process's real parent
//! chain — which is precisely the case that must come back `unknown` rather
//! than `serving`, because no launchd job owns a `cargo test` process.
//!
//! What is defended: the defect that started this — a unit whose port is held
//! by a process its label does not own is never reported as serving; a port
//! nothing listens on is `not_serving` and says which port; a holder whose
//! owning label cannot be read is `unknown` and never `serving`; the command
//! exits non-zero on anything but `serving`; it refuses when no port was
//! named instead of inventing an empty pass; and the declared port is found
//! even when the directory and the host spell the service differently, which
//! is the shape every real placement-backed service has.

mod support;

use support::{live_port, report, stderr, stdout, Fleet, SERVICE};

mod cases;
