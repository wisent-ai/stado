use crate::fixture::{stderr, Journey};
use serde_json::{json, Value};
use std::fs;

/// Deeper than serde_json's default recursion limit, the depth a receipt
/// reached on a workstation that reinstalled one tool a few hundred times.
const NESTING_BEYOND_THE_DEFAULT_LIMIT: usize = 200;

fn receipt(previous: Value) -> Value {
    json!({
        "product": "deep-receipt-journey",
        "surface": "cli",
        "status": "absent",
        "installed_at": "journey",
        "recipe": {},
        "installed_paths": [],
        "backups": [],
        "host": null,
        "source_revision": null,
        "previous": previous,
    })
}

/// Every earlier installation used to nest inside `previous`; once a receipt
/// was deeper than the parser's limit, every product command refused with
/// `recursion limit exceeded`. The receipt is read whole and kept to one level.
#[test]
fn a_receipt_deeper_than_the_parser_limit_does_not_refuse_product_commands() {
    let journey = Journey::new();
    let mut nested = Value::Null;
    for _ in 0..NESTING_BEYOND_THE_DEFAULT_LIMIT {
        nested = receipt(nested);
    }
    let directory = journey.home.join(".stado/products/deep-receipt-journey");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("cli.json"),
        serde_json::to_vec(&nested).unwrap(),
    )
    .unwrap();
    drop(nested);

    let paths = journey.stado(&["product", "paths", "--json"]);
    assert!(
        paths.status.success(),
        "a deep receipt refused stado product: {}",
        stderr(&paths)
    );
    journey.finish();
}
