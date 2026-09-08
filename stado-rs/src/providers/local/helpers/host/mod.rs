//! What this host has to give, measured rather than assumed.
//!
//! [`cpu`] and [`ram`] read the operating system's own counters, [`staging`]
//! measures the disk the agent has already staged onto, and [`job_request`] is
//! the other side of every comparison — what a job asks for.

pub mod cpu;
pub mod job_request;
pub mod ram;
pub mod staging;
