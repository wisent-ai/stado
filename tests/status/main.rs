//! Reads of jobs the run reaper retired: `stado status` by id, `stado machine
//! status` through the run index, and `stado release status` of reaped and
//! failed platforms. They share the `fixture::Store`, so they compile into one
//! test binary and every fixture item is used by the binary that compiles it.
//! Run one with
//! `cargo test --manifest-path tests/status/Cargo.toml --test reaped-reads -- <module>::`.
#[path = "../build_status/failure_lines.rs"]
mod build_status_failure_lines;
#[path = "../build_status/reaped_platform.rs"]
mod build_status_reaped_platform;
mod by_id;
mod fixture;
#[path = "../machine/reaped_read.rs"]
mod machine_reaped_read;
