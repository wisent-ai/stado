//! What `stado serve` settles before its roles start: which registry target
//! this process serves (`identity`), and whether the roles that read the
//! store must wait for this process's own resolver (`own_route`).

pub(super) mod identity;
pub(super) mod own_route;
