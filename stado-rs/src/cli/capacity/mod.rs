//! `stado capacity`: what every host has, what is held on it, and the one
//! way a placed workload takes its hold.
//!
//! `list` is the fleet table read from the capacity publications; the
//! numbers there are already net of live reservations, because the agent
//! subtracts them before it publishes. `reservations` lists the holds
//! themselves. `hold` takes a declared workload's reservation on a host for a
//! fixed time and keeps it heartbeated — the way to keep a host for yourself
//! before you attach, and the way a test proves the primitive end to end.
//! `reserve_for_workload` is the shared path `stado workload attach` and
//! `stado workload run` go through.

mod commands;
mod hold;
mod reserve;

pub use commands::{dispatch, CapacityCommands};
pub use reserve::{reserve_for_workload, ReservationRefusal};
