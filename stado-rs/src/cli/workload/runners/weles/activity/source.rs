//! The activity read's JavaScript, held beside this file as the three ordered
//! parts of one program: host and release inventory, the run walk, and the
//! report the host writes back.

/// Fed to the host's own `node` over the channel's stdin, with the run limit
/// and API port as argv — the same two values the retired bash wrapper took
/// from the host's environment. There is nothing to install on the host and
/// nothing left behind after the read.
///
/// Recordings hold page DOM, console output, HAR bodies, personas and proxy
/// identities. None of that is emitted. What leaves the host is counts,
/// timestamps, run identifiers, artifact sizes, cost, and the pass/fail flag a
/// trajectory wrote about itself — the fields a remote operator view needs to
/// name a run and say how it ended.
pub(crate) const WELES_ACTIVITY_SOURCE: &str = concat!(
    include_str!("source/inventory.js"),
    include_str!("source/runs.js"),
    include_str!("source/report.js"),
);
