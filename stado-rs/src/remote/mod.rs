//! Everything outside this process that Stado talks to: the control plane it
//! answers, the tailnet it addresses hosts on, the object store it reads and
//! writes, and the Azure token those calls carry.

pub mod azure_token;
pub mod control_plane;
pub mod object_store;
pub mod tailnet;
