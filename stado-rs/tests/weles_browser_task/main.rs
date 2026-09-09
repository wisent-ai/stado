//! `stado workload run weles-browser-task` — the admission half, for real.
//!
//! Every case drives the built binary through the surface an operator reaches
//! for, which `stado workload run --help` shows is a workload kind, a
//! `--target` and a `--plan` file. Nothing here starts a browser and nothing
//! here reaches a network: the registry row is this machine, named by the
//! kernel host name, and every case is expected to stop at placement, at the
//! plan document, at the host's own action allowlist, or at the first sentence
//! that is plainly about reaching Weles.
//!
//! What this replaced, and why. The area used to drive `stado host
//! weles-browser-task ... --url ...` and `stado host capability-route`, neither
//! of which this build has: twelve of its thirteen cases failed with clap's
//! `unrecognized subcommand`, and the thirteenth passed on that same error
//! because its only assertion was that a substring was *absent*. It also put a
//! fabricated ssh destination on a row that is this machine, and wrote the
//! allowlist to `~/.config/weles/worker.env` — a path this product does not
//! read. All three are gone: the surface is the real one, the row carries no
//! ssh at all, and the allowlist is the catalog the worker actually reads.
//!
//! Isolation is a tempdir `HOME`, a tempdir local store, a `STADO_CONFIG` that
//! does not exist and `NO_COLOR`, because the tracing report styles
//! `error_code` away from its value otherwise. Every sentence asserted below
//! was copied from a live run on 2026-09-09.

mod allowlist;
mod fixture;
mod placement;
mod plan_document;
mod sign_in;
