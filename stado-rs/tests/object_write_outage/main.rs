//! A store that could not answer is not an object that is absent — on the
//! write plane, where getting it wrong is worse than on a read.
//!
//! `PUT /api/object?...&metadata_only=true` asked `BlobBackend::exists` before
//! attaching metadata, and answered `404 {"state":"absent"}` when it said
//! `false`. The filesystem backend answered `false` for every failure it could
//! not see past: `Path::is_file` is `fs::metadata(..).map(..).unwrap_or(false)`,
//! so a directory this process is refused permission to traverse reads exactly
//! like an empty one. The route then tells its caller the object is gone while
//! the object sits on disk — the same confusion `stado storage stat` refuses by
//! separating `unavailable` from `absent`, arriving at the one place where a
//! caller acts on the answer by writing.
//!
//! Everything here is this machine: the product's own dashboard entry point
//! (`stado::dashboard::serve`, what `stado dashboard --bind … --port …` runs)
//! on a loopback port, a store rooted in a temp directory, and a stand-in
//! Skarbiec broker on loopback so the object boundary can be satisfied without
//! any real vault. The refusal a store cannot see past is produced the way
//! this operating system produces it: a directory whose mode bits deny the
//! owner the traversal, restored on the way out.
//!
//! - [`cases`] the three answers the route owes: an outage that must not be an
//!   absence, an absence that must stay an answer, and a write that must leave
//!   its metadata on disk.
//! - [`dashboard`] the one dashboard every case shares, and the raw HTTP.
//! - [`vault`] the authorization the object routes require.
//!
//! Every status and body asserted below was copied from a live run.

mod cases;
mod dashboard;
mod vault;
