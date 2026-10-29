//! The command a Skarbiec binary writes one canonical item payload with.
//!
//! Skarbiec renamed it: the releases the fleet runs answer `set-json`, and
//! newer ones answer `set` and refuse `set-json`, so a name compiled into
//! Stado was refused `unknown command` by one of the two whichever it named.
//! The binary's own command table (`skarbiec --help`) says which it has, so
//! it is read from the binary that will run the write.

/// The write command `help`, a `skarbiec --help` answer, lists.
pub(crate) fn write_verb(help: &str) -> Result<&'static str, String> {
    let listed = |verb: &str| {
        help.lines()
            .any(|line| line.split_whitespace().next() == Some(verb))
    };
    ["set", "set-json"]
        .into_iter()
        .find(|verb| listed(verb))
        .ok_or_else(|| {
            format!(
                "its command table lists neither `set` nor `set-json`: {}",
                help.trim()
            )
        })
}
