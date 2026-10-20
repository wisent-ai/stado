//! How space is given back and read: the periodic janitor's stated next pass
//! and its kernel lock, `space reclaim`'s second unheld look at a tree, the
//! walk of a full volume under load, `space report`'s one walk of the home,
//! and the run evidence cleaner's boundary at symbolic links.

mod census;
#[path = "../evidence_links.rs"]
mod evidence_links;
#[path = "../load/mod.rs"]
mod load;
mod promise;
mod reclaim;
mod report;
