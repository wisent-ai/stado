use std::path::Path;
use std::process::{Command, Output};

fn fake_products(dir: &Path) {
    let path = dir.join("wisent-products");
    std::fs::write(
        &path,
        r#"#!/bin/sh
printf '{"argv":['
first=1
for arg in "$@"; do
  [ $first -eq 1 ] || printf ','
  first=0
  printf '"%s"' "$arg"
done
printf '],"products":[{"id":"jeden"},{"id":"ster"},{"id":"tama"}]}'
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut mode = std::fs::metadata(&path).unwrap().permissions();
    mode.set_mode(0o755);
    std::fs::set_permissions(path, mode).unwrap();
}

fn stado(dir: &Path, args: &[&str]) -> Output {
    let path = format!("{}:/usr/bin:/bin", dir.display());
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("PATH", path)
        .env("HOME", dir)
        .output()
        .expect("stado runs")
}

#[test]
fn product_catalog_is_the_external_catalog() {
    let dir = tempfile::tempdir().unwrap();
    fake_products(dir.path());
    let out = stado(dir.path(), &["product", "catalog", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["products"][1]["id"], "ster");
    assert_eq!(value["argv"], serde_json::json!(["catalog", "--json"]));
}

#[test]
fn product_install_forwards_surface_and_host_exactly() {
    let dir = tempfile::tempdir().unwrap();
    fake_products(dir.path());
    let out = stado(
        dir.path(),
        &[
            "product",
            "install",
            "weles",
            "--surface",
            "service",
            "--host",
            "mini",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        value["argv"],
        serde_json::json!([
            "install",
            "weles",
            "--surface",
            "service",
            "--host",
            "mini",
            "--json"
        ])
    );
}
