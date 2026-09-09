//! A delivery for a target whose capacity broadcast went stale.

use crate::fixture::ReleaseFixture;
use crate::waits;

/// A target that stopped publishing capacity still gets its delivery queued,
/// pinned to that exact consumer, at the release priority, and keeping the
/// durable output coordinate the delivery's evidence is uploaded to. A
/// delivery silently dropped because its host went quiet is a release that
/// reports success and installs nothing.
///
/// Gated for the same reason as the other two journeys: the signing key lives
/// in a real Skarbiec broker, and the installed 0.2.40 cannot issue the grant.
#[test]
#[ignore = "needs a skarbiec that can issue grants (the installed 0.2.40 answers `unknown command: grant`): build wisent-ai/skarbiec at origin/main with `cargo build --release --locked`, then run `SKARBIEC_TEST_BIN=<skarbiec>/target/release/skarbiec cargo test --test ci-cd -- --ignored`"]
fn stale_target_capacity_still_enqueues_its_exact_release_delivery() {
    let target = "offline-recovery";
    let consumer = "local-offline-recovery.invalid";
    let fixture = ReleaseFixture::start(
        "release-recovery-",
        target,
        Some((target, "offline-recovery.invalid")),
    );
    let mut agent = fixture.spawn_agent();
    waits::wait_for_claimable_capacity(&fixture, &mut agent);
    waits::seed_stale_capacity(&fixture.storage, consumer);

    let mut submit = fixture.spawn_submit("submit");
    let delivery = waits::wait_for_recovery_delivery(&fixture, &mut submit, &mut agent, consumer);
    let _ = submit.kill();
    let _ = submit.wait();
    let _ = agent.kill();
    let _ = agent.wait();

    assert_eq!(delivery["pinned_host"], consumer);
    assert_eq!(delivery["priority"], stado::constants::RELEASE_JOB_PRIORITY);
    assert_eq!(
        delivery["command"],
        stado::constants::PRODUCT_RELEASE_DELIVERY_JOB_COMMAND
    );
    assert!(
        delivery["output_uri"]
            .as_str()
            .is_some_and(|uri| uri.contains("/deliveries/install-on-builder/output")),
        "delivery keeps its durable release output coordinate: {delivery}"
    );
}
