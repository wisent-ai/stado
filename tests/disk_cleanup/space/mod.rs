//! How space is given back and read: the periodic janitor's stated next pass
//! and its kernel lock, `space reclaim`'s second unheld look at a tree, and
//! `space report`'s one walk of the home.

mod promise;
mod reclaim;
mod report;
