//! Everything outside this process that Stado talks to: the control plane it
//! answers, the tailnet and the LAN it addresses hosts on, the object store it
//! reads and writes, the Azure token those calls carry, and the AWS Signature
//! Version 4 both ends of Stado's S3 endpoint compute.

pub mod azure_token;
pub mod control_plane;
pub mod lan;
pub mod object_store;
pub mod sigv4;
pub mod tailnet;
