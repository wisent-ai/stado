mod api;
#[path = "journey/catalog.rs"]
mod catalog;
mod delivery;
mod receipts;
mod runner;

use runner::{require, Configuration, Journey};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn targets(answer: &Value) -> Result<BTreeSet<String>, String> {
    answer["destinations"]
        .as_array()
        .ok_or("destination observations are missing")?
        .iter()
        .map(|destination| {
            destination["target"]
                .as_str()
                .map(str::to_owned)
                .ok_or("destination identity is missing".into())
        })
        .collect()
}

impl Journey {
    fn declare(&mut self, selected: &[String], success: bool) -> Result<Value, String> {
        let mut command = self.command();
        command.args([
            "release",
            "destinations",
            "set",
            &self.configuration.product,
        ]);
        for target in selected {
            command.arg("--target").arg(target);
        }
        self.cli_process(command.arg("--json"), success)
    }

    fn observed_targets(&mut self) -> Result<Value, String> {
        let product = self.configuration.product.clone();
        self.cli(
            &["release", "destinations", "show", &product, "--json"],
            true,
        )
    }

    async fn lifecycle(&mut self) -> Result<(), String> {
        let product = self.configuration.product.clone();
        let selected = self.configuration.targets.clone();
        let expected = selected.iter().cloned().collect::<BTreeSet<_>>();
        let registry = api::command(
            self,
            &["registry", "pull", "--with-generation"],
            false,
            false,
        )
        .await?;
        require(
            registry["document"] == self.registry,
            "the Desktop API must use the same isolated registry as the CLI",
        )?;
        self.cli(
            &["release", "destinations", "show", &product, "--json"],
            false,
        )?;
        self.declare(&selected, true)?;
        let observed = self.observed_targets()?;
        require(
            targets(&observed)? == expected,
            "set did not persist the complete destination set",
        )?;
        let graphical = api::command(
            self,
            &["release", "destinations", "show", &product, "--json"],
            false,
            false,
        )
        .await?;
        require(
            graphical == observed,
            "CLI and Desktop API destination observations differ",
        )?;
        let listed = api::command(
            self,
            &["release", "destinations", "list", "--json"],
            false,
            false,
        )
        .await?;
        let listed = listed["products"]
            .as_array()
            .ok_or("destination list is absent")?
            .iter()
            .find(|item| item["product"] == product)
            .ok_or("listed product is absent")?;
        require(
            targets(listed)? == expected,
            "list did not read the persisted declaration",
        )?;
        api::command(
            self,
            &["release", "destinations", "remove", &product, "--json"],
            false,
            true,
        )
        .await?;
        require(
            self.observed_targets()? == observed,
            "an unconfirmed graphical mutation changed the declaration",
        )?;
        self.declare(&[selected[0].clone(), selected[0].clone()], false)?;
        self.declare(&[uuid::Uuid::new_v4().to_string()], false)?;
        self.declare(&[], false)?;
        require(
            self.observed_targets()? == observed,
            "a refused set changed the declaration or generation",
        )?;
        api::command(
            self,
            &[
                "release",
                "destinations",
                "set",
                &product,
                "--target",
                &selected[0],
                "--json",
            ],
            true,
            false,
        )
        .await?;
        require(
            targets(&self.observed_targets()?)? == BTreeSet::from([selected[0].clone()]),
            "graphical replacement was not persisted",
        )?;
        api::command(
            self,
            &["release", "destinations", "remove", &product, "--json"],
            true,
            false,
        )
        .await?;
        self.cli(
            &["release", "destinations", "show", &product, "--json"],
            false,
        )?;
        let legacy = self.legacy_catalog()?;
        self.catalog(&legacy, "legacy-catalog.json", true)?;
        api::command(
            self,
            &["release", "destinations", "adopt", &product, "--json"],
            true,
            false,
        )
        .await?;
        require(
            targets(&self.observed_targets()?)? == expected,
            "adoption omitted a former destination",
        )?;
        self.declare(&[selected[0].clone()], true)?;
        self.cli(
            &["release", "destinations", "adopt", &product, "--json"],
            false,
        )?;
        require(
            targets(&self.observed_targets()?)? == BTreeSet::from([selected[0].clone()]),
            "adoption overwrote an operator replacement",
        )?;
        self.cli(
            &["release", "destinations", "remove", &product, "--json"],
            true,
        )?;
        let mut dropped = self.manifest.clone();
        dropped["deliveries"]
            .as_array_mut()
            .ok_or("deliveries missing")?
            .pop();
        self.catalog(&dropped, "refused-catalog.json", false)?;
        self.cli(
            &["release", "destinations", "show", &product, "--json"],
            false,
        )?;
        let manifest = self.manifest.clone();
        self.catalog(&manifest, "selector-catalog.json", true)?;
        require(
            targets(&self.observed_targets()?)? == expected,
            "catalog replacement failed to preserve all former hosts",
        )?;
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires isolated real storage, dedicated workers, signing credentials and the native API"]
async fn release_destinations_journey() -> Result<(), String> {
    let path = std::env::var_os("STADO_DESTINATION_JOURNEY")
        .ok_or("STADO_DESTINATION_JOURNEY must name the real qualification configuration")?;
    let configuration: Configuration =
        serde_json::from_slice(&fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = root
        .parent()
        .ok_or("missing checkout root")?
        .join(".wisent-output/release-destinations")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(output.join("tmp")).map_err(|error| error.to_string())?;
    let mut journey = Journey::new(configuration, output);
    let result = async {
        journey.prepare()?;
        delivery::validate_inputs(&journey)?;
        journey.lifecycle().await?;
        delivery::qualify(&mut journey).await?;
        Ok::<(), String>(())
    }
    .await;
    journey.report["passed"] = json!(result.is_ok());
    journey.report["failure"] = json!(result.as_ref().err());
    journey.save()?;
    println!(
        "retained report: {}",
        journey.output.join("report.json").display()
    );
    result
}
