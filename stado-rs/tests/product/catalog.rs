//! The read verb: `stado product catalog` must hand back the external
//! catalogue, unaltered.

use serde_json::Value;

use crate::fixture::{stderr, Delegation};

/// Stado also compiles a service catalogue of its own
/// (`deploy::service_catalog`), generated from the same upstream file, and a
/// command that answered out of it would look right and then drift unnoticed.
/// The assertion is therefore byte equality with what the external executable
/// prints for itself: the only way to reproduce 30 kB of another program's
/// output exactly is to have asked it.
#[test]
fn product_catalog_is_the_real_external_catalog() {
    let journey = Delegation::new();

    let direct = journey.installer(&["catalog", "--json"]);
    assert!(
        direct.status.success(),
        "the real wisent-products refused to print its catalogue: {}",
        stderr(&direct)
    );
    let through_stado = journey.stado(&["product", "catalog", "--json"]);
    assert!(
        through_stado.status.success(),
        "stado product catalog failed: {}",
        stderr(&through_stado)
    );

    assert_eq!(
        String::from_utf8_lossy(&through_stado.stdout),
        String::from_utf8_lossy(&direct.stdout),
        "stado product catalog did not hand back the external catalogue verbatim, so it is \
         answering out of something other than wisent-products"
    );

    let catalog: Value = serde_json::from_slice(&through_stado.stdout)
        .expect("the external catalogue is one JSON document");
    let products = catalog["products"]
        .as_array()
        .expect("the catalogue is a list of products");
    // Every product the rest of Stado names by hand must be in the catalogue
    // it delegates to, or `product install` cannot install what it deploys.
    for expected in ["ster", "stado", "brama", "skarbiec", "weles", "probierz"] {
        assert!(
            products.iter().any(|product| product["id"] == expected),
            "the external catalogue does not hold {expected}"
        );
    }

    // Reading the catalogue is delegation, not local bookkeeping: it must
    // leave no product record behind in the isolated home.
    assert!(
        journey.recorded_products().is_empty(),
        "reading the catalogue recorded installations: {:?}",
        journey.recorded_products()
    );
}
