use crate::common::{atomic_json, slug, Runtime};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Clone, Deserialize, Serialize)]
pub struct Backup {
    pub path: PathBuf,
    pub backup: PathBuf,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct ProductState {
    pub product: String,
    pub surface: String,
    pub status: String,
    pub installed_at: String,
    pub recipe: Value,
    pub installed_paths: Vec<PathBuf>,
    pub backups: Vec<Backup>,
    pub host: Option<String>,
    pub source_revision: Option<String>,
    pub previous: Option<Box<ProductState>>,
    #[serde(default)]
    pub source_directory: Option<PathBuf>,
    #[serde(default)]
    pub release: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

pub fn path(runtime: &Runtime, product: &str, surface: &str) -> Result<PathBuf> {
    slug(product)?;
    slug(surface)?;
    Ok(runtime
        .home
        .join(".stado/products")
        .join(product)
        .join(format!("{surface}.json")))
}

impl ProductState {
    pub fn protected_paths(&self) -> impl Iterator<Item = &PathBuf> {
        let pending = matches!(
            self.status.as_str(),
            "installing" | "removing" | "rolling_back"
        );
        self.installed_paths.iter().chain(
            self.backups
                .iter()
                .filter(move |_| pending)
                .map(|saved| &saved.path),
        )
    }

    pub fn load(runtime: &Runtime, product: &str, surface: &str) -> Result<Option<Self>> {
        let path = path(runtime, product, surface)?;
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("reading receipt {}", path.display()))
            }
        };
        let state: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing receipt {}", path.display()))?;
        if state.product != product || state.surface != surface {
            bail!("receipt identity differs from its path: {}", path.display());
        }
        Ok(Some(state))
    }

    pub fn save(&self, runtime: &Runtime) -> Result<()> {
        atomic_json(
            &path(runtime, &self.product, &self.surface)?,
            &serde_json::to_value(self)?,
        )
    }
}

pub fn all(runtime: &Runtime) -> Result<Vec<ProductState>> {
    let root = runtime.home.join(".stado/products");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut states = Vec::new();
    for product in fs::read_dir(root)? {
        let product = product?;
        if !product.file_type()?.is_dir() {
            continue;
        }
        let id = product
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("product state directory is not UTF-8"))?;
        for entry in fs::read_dir(product.path())? {
            let entry = entry?;
            if !entry.file_type()?.is_file()
                || entry.path().extension().and_then(|s| s.to_str()) != Some("json")
            {
                continue;
            }
            let path = entry.path();
            let surface = path
                .file_stem()
                .and_then(|s| s.to_str())
                .context("receipt filename is not UTF-8")?;
            if let Some(state) = ProductState::load(runtime, &id, surface)? {
                states.push(state);
            }
        }
    }
    Ok(states)
}
