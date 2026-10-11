//! How space is given back: the periodic janitor's stated next pass and its
//! kernel lock, and `space reclaim`'s second unheld look at a tree.

mod promise;
mod reclaim;
