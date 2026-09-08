//! How the sleep argument handed to the remote loop is spelled.

use super::spawns::gap_argument;

#[test]
fn gap_renders_whole_and_fractional_seconds() {
    assert_eq!(gap_argument(1000), "1");
    assert_eq!(gap_argument(2000), "2");
    assert_eq!(gap_argument(500), "0.500");
    assert_eq!(gap_argument(250), "0.250");
}
