//! Two questions the CLI answers directly since 2026-09-19, so the answer is
//! not a file in `~/.oko` grepped afterwards: one part of the registry
//! (`stado registry pull --path`) and one release run (`stado release status
//! --run | --version`).
//!
//! Every test drives the built `stado` binary against a local storage
//! backend under a tempdir, with HOME and STADO_CONFIG isolated the way
//! `registry_cache` isolates them, so the operator's registry, cache and
//! credentials are never read or written.


mod fixture;

#[path = "cases/registry.rs"]
mod registry;
#[path = "cases/runs.rs"]
mod runs;
#[path = "cases/writes.rs"]
mod writes;
