//! The real `stado product catalog` accepts the shipped catalog and refuses
//! the same structured catalog when one service's repair steps become a
//! scalar, or when a store that moves with a service lies outside the
//! account's home. Each refusal names the field rather than running on.

use std::path::PathBuf;

use serde_json::json;

#[test]
fn the_catalog_refuses_scalar_repair_steps() {
    let shipped = include_str!("../../catalog/products.yml");
    let mut invalid = stado_product::catalog::embedded().expect("the shipped catalog is valid");
    let products = invalid["products"]
        .as_array_mut()
        .expect("the catalog holds products");
    let vault = products
        .iter_mut()
        .find(|product| product["id"] == "skarbiec")
        .expect("the vault is catalogued");
    let repair = &mut vault["service"]["answering"]["repair"];
    assert!(repair.is_array(), "the vault declares repair steps");
    *repair = json!("audit-lock");
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../.build/journeys/service-answering-catalog")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&directory).expect("the catalog directory is made");
    let read = |name: &str, body: &str| {
        let path = directory.join(name);
        std::fs::write(&path, body).expect("the catalog copy is written");
        std::process::Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["product", "catalog", "--catalog"])
            .arg(&path)
            .output()
            .expect("stado runs")
    };
    let listed = read("listed.yml", shipped);
    assert!(
        listed.status.success(),
        "the shipped catalog is read: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let malformed = serde_json::to_string(&invalid)
        .expect("the altered catalog is JSON, also accepted as YAML");
    let single = read("single.yml", &malformed);
    let stderr = String::from_utf8_lossy(&single.stderr);
    assert!(
        !single.status.success()
            && stderr.contains("skarbiec: service.answering.repair must be a list of repair steps"),
        "one string where the steps are a list is refused naming the field: {stderr}"
    );
    // A store that moves with a service is a path under the account's home;
    // an absolute one would carry a file the service does not own.
    let mut outside = stado_product::catalog::embedded().expect("the shipped catalog is valid");
    let vault = outside["products"]
        .as_array_mut()
        .expect("the catalog holds products")
        .iter_mut()
        .find(|product| product["id"] == "skarbiec")
        .expect("the vault is catalogued");
    vault["service"]["state"] = json!([{ "path": "/etc/skarbiec.vault.json", "required": true }]);
    let absolute = read(
        "absolute.yml",
        &serde_json::to_string(&outside).expect("the altered catalog is JSON"),
    );
    let stderr = String::from_utf8_lossy(&absolute.stderr);
    assert!(
        !absolute.status.success()
            && stderr.contains("is not a path relative to the account's home"),
        "an absolute store path is refused naming why: {stderr}"
    );
    std::fs::remove_dir_all(&directory).expect("the catalog directory is removed");
}
