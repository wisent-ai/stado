use crate::common::slug;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_BYTES: usize = 64 * 1024;
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub id: String,
    pub name: String,
    pub description: String,
    pub family: String,
    pub visibility: String,
}

#[derive(Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Cli,
    Desktop,
    Web,
    Service,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub surface: Surface,
    pub repository: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub request_id: String,
    pub initiative_id: String,
    pub product: Product,
    pub repositories: Vec<Repository>,
    pub evidence_refs: Vec<String>,
}

pub fn identifier(value: &str) -> Result<()> {
    slug(value)?;
    if value.len() > 100 || !value.as_bytes()[0].is_ascii_lowercase() {
        bail!(
            "creation identities must start with a lowercase letter and contain at most 100 bytes"
        );
    }
    Ok(())
}

impl Request {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            bail!("creation request has an unsupported schema");
        }
        identifier(&self.request_id)?;
        identifier(&self.initiative_id)?;
        identifier(&self.product.id)?;
        if self.product.visibility != "private" {
            bail!("creation requires a private product; public publication is a separate grant");
        }
        if self.product.family != "wisent" && self.product.family != "standalone" {
            bail!("invalid product family");
        }
        if self.product.name.trim().is_empty() || self.product.description.trim().is_empty() {
            bail!("creation requires a non-empty name and description");
        }
        if self.product.description.len() > 350 {
            bail!("creation description exceeds 350 UTF-8 bytes");
        }
        if self.evidence_refs.is_empty()
            || self
                .evidence_refs
                .iter()
                .any(|value| value.trim().is_empty())
        {
            bail!("creation requires non-empty evidence references");
        }
        if self.repositories.is_empty() {
            bail!("creation requires explicit repository surfaces");
        }
        let mut names = HashSet::new();
        let mut surfaces = HashSet::new();
        for row in &self.repositories {
            let parts: Vec<_> = row.repository.split('/').collect();
            if parts.len() != 2 {
                bail!("creation repositories require OWNER/NAME");
            }
            identifier(parts[0])?;
            identifier(parts[1])?;
            if !names.insert(&row.repository) || !surfaces.insert(&row.surface) {
                bail!("creation surfaces and repositories must be unique");
            }
        }
        Ok(())
    }
}
