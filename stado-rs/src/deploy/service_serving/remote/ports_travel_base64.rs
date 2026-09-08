//! What defends the delivered program: the ports it judges travel
//! base64-encoded inside the body rather than in an argument vector, and the
//! owner walk that makes a launcher's child attributable is bounded.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use super::super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_carries_ports_only_base64_and_bounds_the_owner_walk() {
        let script = remote_serving_script(&[58101, 8788]);
        assert!(!script.contains("58101 8788"), "{script}");
        assert!(script.contains(&STANDARD.encode("58101 8788")));
        assert!(script.contains(&MAX_OWNER_DEPTH.to_string()));
        // The walk is what makes a launcher's child attributable to its job.
        assert!(script.contains("resolve_owner"), "{script}");
    }
}
