use crate::fixture::{stderr, Journey};
use std::fs;

/// A host that ran the separate `wisent-products` program keeps its state
/// under `~/.stado/products/wisent-products`: a receipt for its own pipx
/// installation and an onboarding outbox that is no receipt at all. Stado
/// reads that tree as receipts, and before the retirement the outbox refused
/// every product command with `parsing receipt ... missing field product`.
#[test]
fn the_former_programs_state_leaves_the_receipt_tree_and_is_kept() {
    let journey = Journey::new();
    let former = journey.home.join(".stado/products/wisent-products");
    fs::create_dir_all(&former).unwrap();
    let outbox = br#"{"installation_id":"former","pending_events":[]}"#;
    fs::write(former.join("onboarding.json"), outbox).unwrap();

    let paths = journey.stado(&["product", "paths", "--json"]);
    assert!(
        paths.status.success(),
        "a host that ran wisent-products cannot use stado product: {}",
        stderr(&paths)
    );
    assert!(
        !former.try_exists().unwrap(),
        "the former program's state is still read as receipts"
    );
    let retired = journey
        .home
        .join(".local/state/stado/retired/wisent-products/onboarding.json");
    assert_eq!(
        fs::read(&retired).expect("the former program's state was not kept"),
        outbox,
        "the retired outbox differs from what the former program wrote"
    );
    journey.finish();
}
