//! What placement relief decides, read through `stado placement relief`
//! against an isolated fleet: a placed host over its memory watermark with a
//! declared host that has more headroom is a planned move to that host; a
//! stale publication moves nothing; a pressured or smaller candidate is
//! refused by name; and a profile relocated within the cooldown stays.

mod hosts;
mod standby;
mod support;

#[path = "cases/candidates.rs"]
mod candidates;
#[path = "cases/moves.rs"]
mod moves;
#[path = "cases/window.rs"]
mod window;
