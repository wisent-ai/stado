//! The platform stamps: what a host's own log tool says about the sleep it
//! came out of and the interface changes inside the window. One file per
//! platform, because the tool, its output shape and its markers all differ.

mod linux;
mod macos;

pub(super) use linux::{linux_interface_changes, linux_sleep_wake};
pub(super) use macos::{macos_interface_changes, macos_sleep_wake};
