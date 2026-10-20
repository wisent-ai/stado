//! The resolver's held-connection journeys: each drives the shared
//! `held_deployment` fixture, so they compile into one test binary and every
//! fixture item is used by the binary that compiles it. Run one with
//! `cargo test --test service-directory-held -- <module>::`.
mod closed_channel;
mod held;
mod held_answer;
mod held_deployment;
#[path = "../../host_silence/hung_ssh.rs"]
mod hung_ssh;
mod inflight;
mod self_loop;
mod session;
mod silent;
#[path = "../../host_silence/refusal_in_flight/store.rs"]
// The silent journey relays to the store's second object API only.
#[allow(dead_code)]
mod store;
mod takeover;
