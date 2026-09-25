use crate::fixture::{stderr, Journey};
use std::collections::BTreeSet;

#[test]
fn unavailable_surface_and_unknown_product_leave_no_installation() {
    let journey = Journey::new();
    let catalog = journey.catalog();
    let products = catalog["products"]
        .as_array()
        .expect("catalog products are absent");
    let recipes = |product: &serde_json::Value| {
        product["installations"]
            .as_array()
            .expect("catalog recipes are absent")
            .iter()
            .filter_map(|recipe| recipe["surface"].as_str())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    };
    // Every surface some product can be installed on, as the catalog says.
    let surfaces = products.iter().flat_map(recipes).collect::<BTreeSet<_>>();
    let (product, surface) = products
        .iter()
        .find_map(|product| {
            let declared = recipes(product);
            surfaces
                .iter()
                .find(|surface| !declared.contains(*surface))
                .map(|surface| {
                    (
                        product["id"].as_str().expect("product identity is absent"),
                        surface.clone(),
                    )
                })
        })
        .expect("the catalog provides no unavailable surface to exercise");
    let unused_host = format!("unregistered-product-{}", uuid::Uuid::new_v4());
    let unavailable = journey.stado(&[
        "product",
        "install",
        product,
        "--surface",
        &surface,
        "--host",
        &unused_host,
        "--json",
    ]);
    assert!(
        !unavailable.status.success(),
        "an unavailable installation surface was accepted"
    );
    let diagnostic = stderr(&unavailable);
    assert!(
        diagnostic.contains(product) && diagnostic.contains(&surface),
        "the refusal did not identify the actual product and surface: {diagnostic}"
    );
    journey.assert_no_installation();

    let unknown = format!("absent-product-{}", uuid::Uuid::new_v4());
    let refused = journey.stado(&["product", "install", &unknown, "--surface", "cli", "--json"]);
    assert!(!refused.status.success(), "an unknown product was accepted");
    assert!(
        stderr(&refused).contains(&unknown),
        "the refusal did not identify the unknown product: {}",
        stderr(&refused)
    );
    journey.assert_no_installation();
    journey.finish();
}
