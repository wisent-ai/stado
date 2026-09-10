//! Commands addressed at the machines themselves: the agent running on one,
//! the coordinator they answer to, the machine record, and the disk janitor
//! that keeps a host usable.

pub mod agent;
pub mod coordinator;
pub mod disk_cleanup;
pub mod machine;
