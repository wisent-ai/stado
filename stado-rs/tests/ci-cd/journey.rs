//! Build → sign → publish → install, performed for real.

use serde_json::Value;

use crate::fixture::ReleaseFixture;
use crate::waits;

/// The whole release journey: this machine's own builder runs `cargo check`
/// and `cargo build --release` on a committed product, the archive is signed
/// with a key read out of a real Skarbiec broker over its loopback port, the
/// release is published into the local store, the declared delivery installs
/// the staged binary, and the installed binary answers with its own version.
///
/// Gated, not skipped: the broker this needs is a separate product. The
/// installed `skarbiec` on this machine reports 0.2.40 and answers
/// `unknown command: grant`, so it cannot issue the scoped grant the release
/// coordinator reads the signing key with.
#[test]
#[ignore = "needs a skarbiec that can issue grants (the installed 0.2.40 answers `unknown command: grant`): build wisent-ai/skarbiec at origin/main with `cargo build --release --locked`, then run `SKARBIEC_TEST_BIN=<skarbiec>/target/release/skarbiec cargo test --test ci-cd -- --ignored`"]
fn a_real_release_builds_publishes_and_installs_its_binary() {
    let fixture = ReleaseFixture::start("release-", "", None);
    let mut agent = fixture.spawn_agent();
    waits::wait_for_claimable_capacity(&fixture, &mut agent);

    let mut submit = fixture.spawn_submit("submit");
    let status = waits::wait_for_submit(&fixture, &mut submit, &mut agent);
    let report = fixture.read("submit.out");
    let refusal = fixture.read("submit.err");
    let _ = agent.kill();
    let _ = agent.wait();
    assert!(
        status.success(),
        "release submit failed:\nstdout:\n{report}\nstderr:\n{refusal}"
    );

    let release: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(release["state"], "completed");
    assert_eq!(release["platforms"][fixture.platform]["state"], "published");
    assert_eq!(
        release["deliveries"]["install-on-builder"]["state"],
        "passed"
    );
    fixture.assert_installed_probe_answers();
}
