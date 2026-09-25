//! `stado product registry` and `catalog --output` against an isolated copy
//! of the authoritative catalog: add, update and remove one product, keep the
//! operator's layout, and refuse duplicates, dangling integrations and a unit
//! one product runs while another retires it, each without changing the file.

#[path = "../product_support/mod.rs"]
pub mod support;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::fs;
use support::{command, Run};

#[test]
fn registry_lifecycle_preserves_indented_authority_and_rejects_broken_links() -> Result<()> {
    let mut run = Run::new("registry")?;
    let result = journey(&mut run);
    run.finish(result)
}

/// The catalog installer withdraws a product's retired units from a host once
/// its one service is ready, so a unit that one product runs and another
/// retires would be removed by one install and recreated by the next.
#[test]
fn a_service_unit_is_run_or_retired_never_both() -> Result<()> {
    let mut run = Run::new("registry-retired-units")?;
    let result = retired_units(&mut run);
    run.finish(result)
}

fn retired_units(run: &mut Run) -> Result<()> {
    let services = run.root.join("service-catalog.json");
    let output = services
        .to_str()
        .context("service catalog path")?
        .to_owned();
    let cmd = run.product(&["catalog", "--output", &output]);
    command(run, cmd)?.passed()?;
    let derived: Value = serde_json::from_slice(&fs::read(&services)?)?;
    let lake = derived["services"]
        .as_array()
        .context("derived services")?
        .iter()
        .find(|row| row["name"] == "transcript-lake")
        .context("transcript-lake service missing from the derived catalog")?;
    ensure!(
        lake["retired_units"] == serde_json::json!(["com.wisent.transcript-lake-stream"]),
        "the derived service catalog does not carry the retired units: {lake}"
    );

    let before = fs::read(&run.catalog)?;
    for (retired, refusal) in [
        ("com.wisent.always-on.skarbiec", "a declared service unit"),
        ("com.wisent.skarbiec", "already retired by skarbiec"),
        ("com.wisent/../skarbiec", "expected launchd labels"),
    ] {
        let declaration = run.root.join("retiring-service.yaml");
        fs::write(
            &declaration,
            format!(
                "installable: true\nunit: com.wisent.compute.service.transcript-lake\nprogram: $HOME/.local/bin/transcript-lake\nargs: [stream, --json]\nsummary: Retirement refusal journey\nretired_units: [\"{retired}\"]\n"
            ),
        )?;
        let path = declaration.to_str().context("declaration path")?.to_owned();
        let cmd = run.product(&[
            "registry",
            "set",
            "transcript-lake",
            "--service-file",
            &path,
            "--json",
        ]);
        let refused = command(run, cmd)?;
        refused.refused()?;
        let stderr = fs::read_to_string(refused.directory.join("stderr.log"))?;
        ensure!(
            stderr.contains(refusal),
            "retiring {retired} was refused without naming why: {stderr}"
        );
        ensure!(
            fs::read(&run.catalog)? == before,
            "a refused retirement changed the persisted authority"
        );
    }
    Ok(())
}

fn journey(run: &mut Run) -> Result<()> {
    let original = run.authority()?;
    let mut header = original.clone();
    let products = header
        .as_object_mut()
        .context("authority object")?
        .remove("products")
        .context("authority products")?;
    let mut indented = String::from("# Operator catalog annotation must survive mutations.\n");
    indented.push_str(&serde_yaml::to_string(&header)?);
    indented.push_str("products:\n");
    for line in serde_yaml::to_string(&products)?.lines() {
        indented.push_str("  ");
        indented.push_str(line);
        indented.push('\n');
    }
    fs::write(&run.catalog, &indented)?;
    let id = format!("registry-{}", uuid::Uuid::new_v4());
    let add = [
        "registry",
        "add",
        "--id",
        &id,
        "--name",
        "Product registry journey",
        "--owner-repository",
        "wisent-ai/stado",
        "--description",
        "Isolated registry lifecycle",
        "--visibility",
        "private",
        "--family",
        "wisent",
        "--status",
        "preview",
        "--surface",
        "cli=wisent-ai/stado",
        "--evidence",
        "https://stado.wisent.com/docs/builds",
        "--json",
    ];
    let cmd = run.product(&add);
    command(run, cmd)?.passed()?;
    let added = fs::read(&run.catalog)?;
    let cmd = run.product(&add);
    command(run, cmd)?.refused()?;
    ensure!(
        fs::read(&run.catalog)? == added,
        "duplicate registration changed persisted authority"
    );

    let cmd = run.product(&[
        "registry",
        "set",
        &id,
        "--description",
        "Updated through the real Rust CLI",
        "--json",
    ]);
    command(run, cmd)?.passed()?;
    let cmd = run.product(&["catalog", "--json"]);
    let catalog = command(run, cmd)?.json()?;
    let product = rows(&catalog)?
        .iter()
        .find(|row| row["id"] == id)
        .context("new product missing from catalog")?;
    ensure!(
        product["description"] == "Updated through the real Rust CLI",
        "catalog did not expose persisted update"
    );
    let before_refusal = fs::read(&run.catalog)?;
    let missing = format!(
        "missing-{}=https://stado.wisent.com/docs/builds=Unavailable integration",
        uuid::Uuid::new_v4()
    );
    let cmd = run.product(&[
        "registry",
        "set",
        &id,
        "--add-integration",
        &missing,
        "--json",
    ]);
    command(run, cmd)?.refused()?;
    ensure!(
        fs::read(&run.catalog)? == before_refusal,
        "invalid integration damaged the catalog"
    );

    let existing_id = products
        .as_array()
        .context("original products")?
        .first()
        .context("original catalog is empty")?["id"]
        .as_str()
        .context("original product ID")?;
    let link = format!("{id}=https://stado.wisent.com/docs/builds=Registry removal dependency");
    let cmd = run.product(&[
        "registry",
        "set",
        existing_id,
        "--add-integration",
        &link,
        "--json",
    ]);
    command(run, cmd)?.passed()?;
    let linked = fs::read(&run.catalog)?;
    let cmd = run.product(&["registry", "rm", &id, "--yes", "--json"]);
    command(run, cmd)?.refused()?;
    ensure!(
        fs::read(&run.catalog)? == linked,
        "removal left a dangling product integration"
    );
    let cmd = run.product(&[
        "registry",
        "set",
        existing_id,
        "--remove-integration",
        &id,
        "--json",
    ]);
    command(run, cmd)?.passed()?;
    let cmd = run.product(&["registry", "rm", &id, "--yes", "--json"]);
    command(run, cmd)?.passed()?;
    let cmd = run.product(&["catalog", "--json"]);
    let final_catalog = command(run, cmd)?.json()?;
    ensure!(
        !rows(&final_catalog)?.iter().any(|row| row["id"] == id),
        "removed product remains visible"
    );
    ensure!(
        run.authority()? == original,
        "registry lifecycle changed a neighboring product"
    );
    ensure!(
        fs::read_to_string(&run.catalog)?
            .starts_with("# Operator catalog annotation must survive mutations.\n"),
        "registry lifecycle discarded the operator's root annotation"
    );
    Ok(())
}

fn rows(value: &Value) -> Result<&Vec<Value>> {
    value["products"].as_array().context("catalog products")
}
