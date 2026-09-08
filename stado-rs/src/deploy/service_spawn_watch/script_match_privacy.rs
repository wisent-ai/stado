//! The remote program must not be findable by the thing it is searching for.

use super::script::WATCH_SCRIPT;

#[test]
fn the_match_never_reaches_the_remote_argv() {
    // The searcher must not be findable by its own search: `awk` reads the
    // pattern from the environment, so no process command line carries it.
    assert!(WATCH_SCRIPT.contains("ENVIRON[\"STADO_WATCH_MATCH\"]"));
    assert!(!WATCH_SCRIPT.contains("awk -v m="));
}
