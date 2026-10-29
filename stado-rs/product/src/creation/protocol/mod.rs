pub mod distinct;

use crate::catalog::{Family, Visibility};
use crate::common::slug;
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub id: String,
    pub name: String,
    pub description: String,
    pub family: Family,
    pub visibility: Visibility,
}

/// The surface kinds a repository is provisioned for; each word is the
/// catalog's `surfaces[].kind` for that surface.
#[derive(Deserialize, Serialize, PartialEq, Eq, Hash, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Cli,
    Desktop,
    Web,
    Service,
    Ios,
    Android,
}

impl Surface {
    /// `word` as a surface kind, or serde's refusal listing the kinds.
    pub fn parse(word: &str) -> Result<Self, String> {
        Self::deserialize(Value::from(word)).map_err(|error| error.to_string())
    }
}

impl std::fmt::Display for Surface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::word::write(self, formatter)
    }
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

const REQUEST_FIELDS: &[&str] = &[
    "schema_version",
    "request_id",
    "initiative_id",
    "product",
    "repositories",
    "evidence_refs",
];
const PRODUCT_FIELDS: &[&str] = &["id", "name", "description", "family", "visibility"];
const REPOSITORY_FIELDS: &[&str] = &["surface", "repository"];

/// `field` names what is refused; `value` must be a creation identity.
pub fn identifier(field: &str, value: &str) -> Result<()> {
    slug(value).with_context(|| format!("{field}: {value:?} is not a creation identity"))?;
    if value.len() > 100 || !value.as_bytes()[0].is_ascii_lowercase() {
        bail!(
            "{field}: {value:?} is not a creation identity: creation identities start with a \
             lowercase letter and contain at most 100 bytes"
        );
    }
    Ok(())
}

/// `OWNER/NAME`, each part a creation identity; `field` names the refusal.
pub fn repository(field: &str, value: &str) -> Result<()> {
    let Some((owner, name)) = value
        .split_once('/')
        .filter(|(_, name)| !name.contains('/'))
    else {
        bail!("{field}: {value:?} is not OWNER/NAME");
    };
    identifier(field, owner)?;
    identifier(field, name)
}

/// The JSON object `name` holding exactly `fields`, each named in a refusal
/// as `prefix` and the field (`product.family`; bare at the top).
fn object<'a>(
    value: &'a Value,
    name: &str,
    prefix: &str,
    fields: &[&str],
) -> Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .with_context(|| format!("{name}: expected a JSON object, found {value}"))?;
    if let Some(key) = object.keys().find(|key| !fields.contains(&key.as_str())) {
        bail!(
            "{prefix}{key}: unknown field; {name} holds {}",
            fields.join(", ")
        );
    }
    if let Some(missing) = fields.iter().find(|field| !object.contains_key(**field)) {
        bail!(
            "{prefix}{missing}: missing; {name} holds {}",
            fields.join(", ")
        );
    }
    Ok(object)
}

/// `object[name]` as `T`, a refusal naming `prefix` and the field with
/// serde's reason.
fn field<T: DeserializeOwned>(object: &Map<String, Value>, prefix: &str, name: &str) -> Result<T> {
    T::deserialize(&object[name]).map_err(|error| anyhow::anyhow!("{prefix}{name}: {error}"))
}

impl Request {
    /// Read a request file's bytes: every refusal names the field it is
    /// about (`product.family`, `repositories[1].surface`), what it found
    /// and what the field accepts, before anything is checked elsewhere.
    pub fn parse(raw: &[u8]) -> Result<Self> {
        let value: Value =
            serde_json::from_slice(raw).context("the creation request is not JSON")?;
        let top = object(&value, "the request", "", REQUEST_FIELDS)?;
        let product = object(&top["product"], "product", "product.", PRODUCT_FIELDS)?;
        let rows = top["repositories"].as_array().with_context(|| {
            format!(
                "repositories: expected a list, found {}",
                top["repositories"]
            )
        })?;
        let mut repositories = Vec::with_capacity(rows.len());
        for (index, row) in rows.iter().enumerate() {
            let name = format!("repositories[{index}]");
            let prefix = format!("{name}.");
            let row = object(row, &name, &prefix, REPOSITORY_FIELDS)?;
            repositories.push(Repository {
                surface: field(row, &prefix, "surface")?,
                repository: field(row, &prefix, "repository")?,
            });
        }
        let request = Self {
            schema_version: field(top, "", "schema_version")?,
            request_id: field(top, "", "request_id")?,
            initiative_id: field(top, "", "initiative_id")?,
            product: Product {
                id: field(product, "product.", "id")?,
                name: field(product, "product.", "name")?,
                description: field(product, "product.", "description")?,
                family: field(product, "product.", "family")?,
                visibility: field(product, "product.", "visibility")?,
            },
            repositories,
            evidence_refs: field(top, "", "evidence_refs")?,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != SCHEMA_VERSION {
            bail!(
                "schema_version: {} is not supported; this Stado reads creation requests of \
                 schema {SCHEMA_VERSION}",
                self.schema_version
            );
        }
        identifier("request_id", &self.request_id)?;
        identifier("initiative_id", &self.initiative_id)?;
        identifier("product.id", &self.product.id)?;
        if self.product.visibility != Visibility::Private {
            bail!(
                "product.visibility: creation requires private; public publication is a \
                 separate grant"
            );
        }
        if self.product.name.trim().is_empty() {
            bail!("product.name: empty; a product needs its display name");
        }
        if self.product.description.trim().is_empty() {
            bail!("product.description: empty; a product needs what it does in one sentence");
        }
        // GitHub's limit for a repository description, which every
        // provisioned repository receives.
        if self.product.description.len() > 350 {
            bail!(
                "product.description: {} UTF-8 bytes; a repository description holds at most 350",
                self.product.description.len()
            );
        }
        if self.evidence_refs.is_empty() {
            bail!("evidence_refs: empty; creation requires at least one evidence reference");
        }
        if let Some(index) = self
            .evidence_refs
            .iter()
            .position(|value| value.trim().is_empty())
        {
            bail!("evidence_refs[{index}]: empty; every evidence reference names its source");
        }
        if self.repositories.is_empty() {
            bail!("repositories: empty; creation requires explicit repository surfaces");
        }
        let mut names = HashMap::new();
        let mut surfaces = HashMap::new();
        for (index, row) in self.repositories.iter().enumerate() {
            let path = format!("repositories[{index}]");
            repository(&format!("{path}.repository"), &row.repository)?;
            if let Some(first) = names.insert(row.repository.as_str(), index) {
                bail!(
                    "{path}.repository: {} is already repositories[{first}]; each repository is \
                     provisioned once",
                    row.repository
                );
            }
            if let Some(first) = surfaces.insert(row.surface, index) {
                bail!(
                    "{path}.surface: {} is already repositories[{first}]; a product has one \
                     surface of each kind",
                    row.surface
                );
            }
        }
        Ok(())
    }
}
