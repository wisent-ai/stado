//! When did anyone last look? The age of a fact, kept in the data model
//! instead of in whichever command happened to check.
//!
//! The service directory declared `stado-object-api` active on a laptop. That
//! declaration was structurally valid for twelve days: `config validate`
//! passed, `registry validate` passed, `doctor` passed, and the schema had
//! nothing to say about it because nothing in the schema was wrong. The lid
//! was closed. Every consumer routed through a forward with no upstream, and
//! the worker on the always-on Mac refused 29,616 times to claim work whose
//! diagnostics it could not upload.
//!
//! The defect was not that the fleet believed something false. It is that the
//! model could not express the sentence "nobody has looked since". `active` was
//! stored as a declaration, declarations do not decay, and so a statement
//! entered twelve days earlier read exactly like a statement confirmed a second
//! ago. There was no field that could have been wrong, which is why no
//! validator could have caught it.
//!
//! So an observation is a separate kind of record from a declaration, and it
//! carries four things a declaration does not:
//!
//!   fact     what was being checked, named the same way by every checker, so
//!            two commands looking at one thing produce one row and not two
//!   vantage  who looked. Reachability has no fleet-wide answer -- a loopback
//!            endpoint is true from its own host and false from everywhere
//!            else -- so an observation without a vantage is not a smaller
//!            observation, it is an unusable one
//!   state    `observed`, `unreachable`, or `unverified`
//!   at       when. This is the field the outage needed and did not have
//!
//! Three states, never two. `unreachable` means someone looked and it was not
//! there. `unverified` means the look did not happen: host down, helper not
//! installed, channel refused. Collapsing those into one `false` is how a fleet
//! learns to ignore its own reports -- an uninstalled probe starts rendering as
//! an outage, operators learn that red does not mean red, and the one real
//! outage in the pile reads like the rest of the noise. Twelve days is how long
//! that takes.
//!
//! And `Never` is the fourth answer, the one that has to stay distinct from all
//! three: this fleet has no record of anyone ever checking this. Reading that
//! as `observed` is the original bug. Reading it as `unreachable` invents an
//! outage. It is neither; it is an admission, and the only honest rendering of
//! it is the word `never`.
//!
//! Freshness is therefore a property of the record and not of the reader. A
//! `Fresh` observation is one made inside the caller's TTL and may be acted on.
//! `Stale` still carries the observation -- the last thing anyone saw is worth
//! showing, clearly marked as history -- but it must never be treated as the
//! present. [`DEFAULT_TTL`] is one hour because a laptop lid closes in a
//! second and an hour is how long the fleet is willing to be wrong about it.
//!
//! Storage is `~/.stado/observations.json`, owner-only, written through a
//! temporary file in the same directory and a rename, the same discipline
//! `cli::directory::write_forward_marker` uses for forward markers: a reader
//! must never see half a file, and a reader here is a routing decision.

mod display;
mod observation;
mod staleness;
mod store;

pub use display::{describe, describe_in, render};
pub use observation::{service_fact, Observation, MISOWNED, OBSERVED, UNREACHABLE, UNVERIFIED};
pub use staleness::{freshness, freshness_in, Freshness, DEFAULT_TTL};
pub use store::{load, record};
