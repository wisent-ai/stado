use crate::fixture::Journey;
use stado::cli::setup::product::Surface;

#[test]
fn native_catalog_exposes_the_stado_cli_without_installing_a_surface() {
    let journey = Journey::new();
    let catalog = journey.catalog();
    let products = catalog["products"]
        .as_array()
        .expect("catalog products are absent");
    let product = products
        .iter()
        .find(|product| product["id"] == env!("CARGO_PKG_NAME"))
        .expect("the fleet's product catalog cannot install its own Stado CLI");
    assert!(
        product["installations"]
            .as_array()
            .expect("installation recipes are absent")
            .iter()
            .any(|recipe| recipe["surface"] == Surface::Cli.as_str()),
        "the real catalog omitted the Stado CLI installation recipe"
    );
    journey.assert_no_installation();
    journey.finish();
}
