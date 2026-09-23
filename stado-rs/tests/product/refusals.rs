use crate::fixture::{stderr, Journey};
use clap::ValueEnum;
use stado::cli::setup::product::Surface;

#[test]
fn unavailable_surface_and_unknown_product_leave_no_installation() {
    let journey = Journey::new();
    let catalog = journey.catalog();
    let products = catalog["products"]
        .as_array()
        .expect("catalog products are absent");
    let (product, surface) = products
        .iter()
        .find_map(|product| {
            let recipes = product["installations"]
                .as_array()
                .expect("catalog recipes are absent");
            Surface::value_variants()
                .iter()
                .find(|surface| {
                    !recipes
                        .iter()
                        .any(|recipe| recipe["surface"] == surface.as_str())
                })
                .map(|surface| {
                    (
                        product["id"].as_str().expect("product identity is absent"),
                        surface.as_str(),
                    )
                })
        })
        .expect("the published catalog provides no unavailable surface to exercise");
    let unused_host = format!("unregistered-product-{}", uuid::Uuid::new_v4());
    let unavailable = journey.stado(&[
        "product",
        "install",
        product,
        "--surface",
        surface,
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
        diagnostic.contains(product) && diagnostic.contains(surface),
        "the refusal did not identify the actual product and surface: {diagnostic}"
    );
    journey.assert_no_installation();

    let unknown = format!("absent-product-{}", uuid::Uuid::new_v4());
    let refused = journey.stado(&[
        "product",
        "install",
        &unknown,
        "--surface",
        Surface::Cli.as_str(),
        "--json",
    ]);
    assert!(!refused.status.success(), "an unknown product was accepted");
    assert!(
        stderr(&refused).contains(&unknown),
        "the refusal did not identify the unknown product: {}",
        stderr(&refused)
    );
    journey.assert_no_installation();
    journey.finish();
}
