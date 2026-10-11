//! Before any delivery runs: the candidate binary identifies the exact clean
//! source revision, the test home, registry and workers are isolated from
//! the operator's, and the real catalog and release stores start empty.

use super::{require, Journey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

impl Journey {
    pub fn prepare(&mut self) -> Result<(), String> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("repository root is missing")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let revision = self.execute(
            Path::new("git"),
            &[
                "-C",
                root.to_str().ok_or("non-UTF8 repository")?,
                "rev-parse",
                "HEAD",
            ],
            false,
            true,
        )?;
        let revision = revision.trim();
        self.execute(
            Path::new("git"),
            &["-C", root.to_str().unwrap(), "diff", "--quiet", "HEAD"],
            false,
            true,
        )?;
        let binary = self.binary.clone();
        let version = self.execute(&binary, &["--version"], false, true)?;
        let reported_revision = version
            .split_whitespace()
            .map(|part| part.trim_matches(['(', ')']))
            .find(|part| part.len() == 40 && part.bytes().all(|byte| byte.is_ascii_hexdigit()));
        require(
            reported_revision == Some(revision),
            "the candidate must identify the exact clean source revision",
        )?;
        let mut file = fs::File::open(&binary).map_err(|error| error.to_string())?;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 8192];
        loop {
            let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        self.report["source_revision"] = json!(revision);
        self.report["binary_sha256"] = json!(format!("{:x}", digest.finalize()));
        self.configuration.home = self
            .configuration
            .home
            .canonicalize()
            .map_err(|error| error.to_string())?;
        self.configuration.config = self
            .configuration
            .config
            .canonicalize()
            .map_err(|error| error.to_string())?;
        self.configuration.source = self
            .configuration
            .source
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let operator_home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is missing")?)
            .canonicalize()
            .map_err(|error| error.to_string())?;
        require(
            self.configuration.home != operator_home,
            "the operator home cannot be a test home",
        )?;
        require(
            self.configuration.source.parent() == root.parent(),
            "use the existing canonical product checkout, not a nested checkout",
        )?;
        let source = self
            .configuration
            .source
            .to_str()
            .ok_or("non-UTF8 source")?
            .to_owned();
        let coordinate = format!("{}:.wisent-release.json", self.configuration.commit);
        let manifest = self.execute(
            Path::new("git"),
            &["-C", &source, "show", &coordinate],
            false,
            true,
        )?;
        self.manifest = serde_json::from_str(&manifest).map_err(|error| error.to_string())?;
        require(
            self.manifest["product"] == self.configuration.product,
            "source and configured products differ",
        )?;
        let operator = self.execute(
            &binary,
            &["registry", "pull", "--with-generation"],
            false,
            true,
        )?;
        let operator: Value = serde_json::from_str(&operator).map_err(|error| error.to_string())?;
        let isolated = self.cli(&["registry", "pull", "--with-generation"], true)?;
        let repeated = self.execute(
            &binary,
            &["registry", "pull", "--with-generation"],
            false,
            true,
        )?;
        let repeated: Value = serde_json::from_str(&repeated).map_err(|error| error.to_string())?;
        require(
            operator == repeated
                && isolated["document"] != operator["document"]
                && isolated["generation"] != operator["generation"],
            "test registry must differ from a stable observed operator authority",
        )?;
        let production = operator["document"]["targets"]
            .as_array()
            .ok_or("operator registry targets are unavailable")?;
        let targets = isolated["document"]["targets"]
            .as_array()
            .ok_or("test registry targets are unavailable")?;
        require(
            self.configuration.targets.len() > 1,
            "use multiple real dedicated workers",
        )?;
        for name in &self.configuration.targets {
            let target = targets
                .iter()
                .find(|target| target["name"] == *name)
                .ok_or("a test worker is not registered")?;
            let ssh = target["ssh"]
                .as_str()
                .ok_or("test workers need explicit isolated account identities")?;
            let (user, host) = ssh
                .rsplit_once('@')
                .ok_or("test workers need explicit SSH account names")?;
            require(
                !user.is_empty()
                    && !host.is_empty()
                    && !production.iter().any(|production| {
                        production["ssh"]
                            .as_str()
                            .and_then(|ssh| ssh.rsplit_once('@'))
                            .is_some_and(|(production_user, _)| {
                                user.eq_ignore_ascii_case(production_user)
                            })
                            || target["account_ref"]
                                .as_str()
                                .is_some_and(|account| production["account_ref"] == account)
                    }),
                "use dedicated test account identities, not operator accounts or their aliases",
            )?;
        }
        require(
            isolated["document"]["release_delivery_targets"]
                .get(&self.configuration.product)
                .is_none(),
            "use an isolated product with no preexisting destination declaration",
        )?;
        self.registry = isolated["document"].clone();
        self.report["registry_location"] = isolated["location"].clone();
        self.report["product_commit"] = json!(self.configuration.commit);
        for (namespace, prefix) in [
            (
                "system",
                format!("release-catalog/{}.json", self.configuration.product),
            ),
            ("releases", format!("{}/", self.configuration.product)),
        ] {
            let before = self.objects(namespace, &prefix, false)?;
            let isolated = self.objects(namespace, &prefix, true)?;
            let after = self.objects(namespace, &prefix, false)?;
            require(before == after && before["objects"].as_array().is_some_and(|objects| !objects.is_empty())
                    && isolated["objects"].as_array().is_some_and(Vec::is_empty),
                    "the real catalog and release stores must be empty and separate from stable operator stores")?;
        }
        self.save()
    }
}
