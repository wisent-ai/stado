mod prepared;

use super::core::{absolute, command};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::Cursor,
    path::{Path, PathBuf},
};

#[derive(Clone, Default)]
pub struct Policy {
    entitlements: Option<(PathBuf, plist::Value)>,
    hardened_runtime: bool,
    boolean_entitlements: BTreeMap<String, bool>,
}

pub fn entitlements(path: &Path) -> Result<plist::Value> {
    let path = absolute(path)?;
    let observed = command(
        "/usr/bin/codesign",
        &[
            "--display",
            "--entitlements",
            "-",
            "--xml",
            path.to_str().context("non-UTF8 signing target")?,
        ],
        true,
    )?;
    if observed.stdout.iter().all(u8::is_ascii_whitespace) {
        return Ok(plist::Value::Dictionary(plist::Dictionary::new()));
    }
    let value = plist::Value::from_reader(Cursor::new(observed.stdout))
        .with_context(|| format!("read the actual signed entitlements of {}", path.display()))?;
    ensure!(
        value.as_dictionary().is_some(),
        "signed entitlements are not a dictionary: {}",
        path.display()
    );
    Ok(value)
}

impl Policy {
    pub fn new(
        entitlements: Option<&Path>,
        hardened_runtime: bool,
        boolean_values: &[String],
    ) -> Result<Self> {
        let mut boolean_entitlements = BTreeMap::new();
        for value in boolean_values {
            let (key, requested) = value
                .split_once('=')
                .context("boolean entitlement requires NAME=true or NAME=false")?;
            ensure!(!key.is_empty(), "boolean entitlement name cannot be empty");
            let requested = requested
                .parse::<bool>()
                .context("boolean entitlement value must be true or false")?;
            ensure!(
                boolean_entitlements
                    .insert(key.to_owned(), requested)
                    .is_none(),
                "boolean entitlement was specified more than once: {key}"
            );
        }
        let entitlements = entitlements
            .map(|path| -> Result<_> {
                let path = absolute(path)?.canonicalize()?;
                path.to_str().context("non-UTF8 entitlement path")?;
                let value = plist::Value::from_file(&path)
                    .with_context(|| format!("read requested entitlements {}", path.display()))?;
                ensure!(
                    value.as_dictionary().is_some(),
                    "entitlements require a plist dictionary: {}",
                    path.display()
                );
                Ok((path, value))
            })
            .transpose()?;
        Ok(Self {
            entitlements,
            hardened_runtime,
            boolean_entitlements,
        })
    }
    pub fn preserves_metadata(&self) -> bool {
        self.entitlements.is_none()
            && !self.hardened_runtime
            && self.boolean_entitlements.is_empty()
    }

    pub fn arguments<'a>(&'a self, args: &mut Vec<&'a str>) -> Result<()> {
        match (self.entitlements.is_some(), self.hardened_runtime) {
            (false, false) => args.push("--preserve-metadata=entitlements,flags"),
            (true, false) => args.push("--preserve-metadata=flags"),
            (false, true) => args.push("--preserve-metadata=entitlements"),
            (true, true) => {}
        }
        if let Some((path, _)) = &self.entitlements {
            args.extend([
                "--entitlements",
                path.to_str().context("non-UTF8 entitlement path")?,
            ]);
        }
        if self.hardened_runtime {
            args.extend(["--options", "runtime"]);
        }
        Ok(())
    }

    pub fn satisfied(&self, path: &Path, report: &mut Value) -> Result<bool> {
        let mut satisfied = !self.hardened_runtime || report["hardened_runtime"] == true;
        if let Some((_, expected)) = &self.entitlements {
            let observed = entitlements(path)?;
            report["entitlements"] = serde_json::to_value(&observed)?;
            satisfied &= &observed == expected;
        }
        Ok(satisfied)
    }

    pub fn verify(&self, path: &Path, report: &mut Value) -> Result<()> {
        ensure!(self.satisfied(path, report)?,
            "signed policy differs from the request: target {}, hardened runtime requested {}, entitlements requested {:?}, observed {}",
            path.display(), self.hardened_runtime, self.entitlements.as_ref().map(|(_, value)| value), report);
        Ok(())
    }
}
