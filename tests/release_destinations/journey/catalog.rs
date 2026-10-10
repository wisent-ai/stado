use super::runner::{require, Journey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

impl Journey {
    pub(super) fn catalog(
        &mut self,
        manifest: &Value,
        name: &str,
        success: bool,
    ) -> Result<Value, String> {
        let source = self
            .configuration
            .source
            .to_str()
            .ok_or("non-UTF8 source")?
            .to_owned();
        let repository = self.execute(
            Path::new("git"),
            &["-C", &source, "remote", "get-url", "origin"],
            false,
            true,
        )?;
        let bytes = serde_json::to_vec(manifest).map_err(|error| error.to_string())?;
        let catalog = json!({"repositories": [{
            "repository": repository.trim(), "product": self.configuration.product,
            "manifest": manifest, "manifest_sha256": format!("{:x}", Sha256::digest(bytes)),
        }]});
        let path = self.output.join(name);
        fs::write(
            &path,
            serde_json::to_vec_pretty(&catalog).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        self.cli(
            &[
                "release",
                "catalog",
                "sync",
                "--catalog",
                path.to_str().ok_or("non-UTF8 catalog path")?,
                "--json",
            ],
            success,
        )
    }

    pub(super) fn legacy_catalog(&self) -> Result<Value, String> {
        let mut legacy = self.manifest.clone();
        let mut deliveries = Vec::new();
        let registry = self.registry["targets"]
            .as_array()
            .ok_or("registry targets are missing")?;
        for delivery in self.manifest["deliveries"]
            .as_array()
            .ok_or("source has no deliveries")?
        {
            require(
                delivery["target"]["product"] == self.configuration.product,
                "qualification source must use its own registry selector for every delivery",
            )?;
            require(
                delivery
                    .get("after")
                    .is_none_or(|after| after.as_array().is_some_and(Vec::is_empty)),
                "legacy adoption qualification needs independently installed source deliveries",
            )?;
            for name in &self.configuration.targets {
                let target = registry
                    .iter()
                    .find(|target| target["name"] == *name)
                    .ok_or("target is missing")?;
                if target["release_platform"] == delivery["runner_platform"] {
                    let mut pinned = delivery.clone();
                    pinned["target"] = json!(name);
                    pinned["name"] = json!(format!(
                        "{}--{name}",
                        delivery["name"].as_str().ok_or("delivery name missing")?
                    ));
                    deliveries.push(pinned);
                }
            }
        }
        require(
            !deliveries.is_empty(),
            "no source installation can run on the dedicated workers",
        )?;
        legacy["deliveries"] = json!(deliveries);
        Ok(legacy)
    }
}
