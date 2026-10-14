//! A resident CLI installation is not done when files have been placed:
//! its managed process must answer its declared port on the new image.

use std::fs::File;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::common::{checked, stado, Runtime};
use crate::install::{operation, transaction};
use crate::state::ProductState;

pub(super) struct Activation<'a> {
    pub runtime: &'a Runtime,
    pub product: &'a Value,
    pub id: &'a str,
    pub surface: &'a str,
    pub host: Option<&'a str>,
    pub resident_host: Option<&'a str>,
    pub recipe: &'a Value,
    pub lock: &'a Path,
}

impl Activation<'_> {
    pub fn finish(self, installed: &mut ProductState, writer: File) -> Result<File> {
        let after = operation::after_install(
            self.runtime,
            (self.id, self.surface, self.host),
            self.recipe,
            installed,
            self.lock,
            writer,
        );
        let writer = match after {
            Ok(writer) => writer,
            Err(cause) => {
                return match self.resident_host {
                    Some(host) => self.restore(host, cause, installed),
                    None => Err(cause),
                };
            }
        };
        let Some(host) = self.resident_host else {
            return Ok(writer);
        };
        match self.await_serving(host) {
            Ok(observed) => {
                installed
                    .extra
                    .insert("resident_activation".into(), observed);
                installed.save(self.runtime)?;
                Ok(writer)
            }
            Err(cause) => {
                drop(writer);
                self.restore(host, cause, installed)
            }
        }
    }

    fn await_serving(&self, host: &str) -> Result<Value> {
        let output = checked(stado().args([
            "service", "serving", self.id, "--host", host, "--wait", "--json",
        ]))
        .with_context(|| {
            format!(
                "{} on {host} did not serve its declared port after replacement",
                self.id
            )
        })?;
        let observed: Value = serde_json::from_slice(&output.stdout).with_context(|| {
            format!(
                "{} on {host} served but returned invalid serving evidence",
                self.id
            )
        })?;
        let rows = observed
            .as_array()
            .context("serving returned no unit rows")?;
        if rows.is_empty() || rows.iter().any(|row| row["serving"] != "serving") {
            bail!(
                "{} on {host} did not serve every declared port: {observed}",
                self.id
            );
        }
        Ok(json!({
            "host": host,
            "operation": "stado service serving --wait",
            "observed": observed
        }))
    }

    fn restore(&self, host: &str, cause: anyhow::Error, expected: &ProductState) -> Result<File> {
        let restored = transaction::lifecycle::rollback_expected(
            self.runtime,
            self.product,
            self.surface,
            expected,
        );
        let restored = match restored {
            Ok(restored) => restored,
            Err(error) => bail!(
                "{} activation failed: {cause:#}; exact-backup rollback failed: {error:#}",
                self.id
            ),
        };
        let restarted = checked(stado().args(["service", "restart", self.id, "--host", host]));
        if let Err(error) = restarted {
            bail!(
                "{} activation failed: {cause:#}; exact backup restored as {:?}, but its unit did not restart: {error:#}",
                self.id,
                restored.status
            );
        }
        match self.await_serving(host) {
            Ok(observed) => bail!(
                "{} activation failed: {cause:#}; exact backup restored as {:?} and its resident unit serves: {observed}",
                self.id,
                restored.status
            ),
            Err(error) => bail!(
                "{} activation failed: {cause:#}; exact backup restored as {:?}, but its resident unit is not serving: {error:#}",
                self.id,
                restored.status
            ),
        }
    }
}
