use crate::Journey;
use anyhow::{Context, Result};
use serde_json::json;
use stado_product::{cargo_lock, export_committed_source};
use std::{fs, path::Path};

impl Journey {
    pub fn changed_requirement(
        &mut self,
        checkout: &Path,
        manifest: &str,
        version: &str,
        edition: &str,
    ) -> Result<()> {
        let mut changed = semver::Version::parse(version)?;
        changed.pre = semver::Prerelease::new("changed")?;
        anyhow::ensure!(
            changed.to_string() != version,
            "fixture must change the locked version"
        );
        fs::write(
            checkout.join("dependency/Cargo.toml"),
            format!("[package]\nname = \"lock-dependency\"\nversion = \"{changed}\"\nedition = \"{edition}\"\n"),
        )?;
        fs::write(
            checkout.join("Cargo.toml"),
            format!("{manifest}lock-dependency = {{ path = \"dependency\", version = \"={changed}\" }}\n"),
        )?;
        let revision = self.commit(checkout)?;
        let tree = self.root.join("version-export");
        export_committed_source(checkout, &revision, &tree)?;
        let lock = fs::read(tree.join("Cargo.lock"))?;
        let failure = self
            .build(&tree, &revision)
            .expect_err("same-name incompatible locked version must fail");
        let source = failure
            .downcast_ref::<cargo_lock::UnlockedManifest>()
            .with_context(|| format!("version mismatch was not attributed: {failure:#}"))?;
        assert_eq!(source.revision, revision);
        assert_eq!(source.manifest, Path::new("Cargo.toml"));
        assert!(source.dependencies.iter().any(|detail| {
            detail.contains("lock-dependency")
                && detail.contains(&format!("={changed}"))
                && detail.contains(&format!("locked {version};"))
                && detail.contains("declared in Cargo.toml")
        }));
        assert_eq!(
            fs::read(tree.join("Cargo.lock"))?,
            lock,
            "version diagnosis must not rewrite the export lock"
        );
        self.report["version_refusal"] = json!(format!("{failure:#}"));
        let cargo = std::env::var("CARGO")?;
        self.run(checkout, &cargo, &["generate-lockfile", "--offline"])?;
        let repaired = self.commit(checkout)?;
        let current = self.root.join("version-current-export");
        export_committed_source(checkout, &repaired, &current)?;
        self.build(&current, &repaired)?;
        let binary = current.join("target/debug/lock-journey");
        assert_eq!(
            self.run(&current, binary.to_str().context("binary path")?, &[])?,
            "locked dependency executed"
        );
        Ok(())
    }
}
