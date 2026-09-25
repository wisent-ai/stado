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
        // Receipts written before history was capped nest every earlier
        // installation in `previous`; a tool reinstalled a few hundred times
        // is deeper than serde_json's default limit, and one such file used to
        // refuse every product command. Read it whole, keep one level.
        let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
        deserializer.disable_recursion_limit();
        let state = Self::deserialize(&mut deserializer)
            .and_then(|state| deserializer.end().map(|()| state))
            .with_context(|| format!("parsing receipt {}", path.display()))?
            .with_one_previous();
        if state.product != product || state.surface != surface {
            bail!("receipt identity differs from its path: {}", path.display());
        }
        Ok(Some(state))
    }

    /// Rollback reads only the state directly before this one, so that is
    /// all a receipt keeps.
    pub fn with_one_previous(mut self) -> Self {
        if let Some(previous) = self.previous.as_mut() {
            previous.previous = None;
        }
        self
    }

    /// This state as the `previous` of the next one: without its own history.
    pub fn without_previous(mut self) -> Self {
        self.previous = None;
        self
    }

    pub fn save(&self, runtime: &Runtime) -> Result<()> {
        atomic_json(
            &path(runtime, &self.product, &self.surface)?,
            &serde_json::to_value(self)?,
        )
    }
}

/// The state the separate `wisent-products` program kept about itself: a
/// receipt for its own pipx installation and its onboarding outbox. That
/// program is Stado now and the catalog holds no such product, so the
/// directory is not a set of receipts; read as one, its outbox refused every
/// install on the hosts it ran on. It moves once, out of the receipt tree,
/// the way `creation::state` moves the former creation journal; nothing is
/// deleted.
const FORMER_PROGRAM: &str = "wisent-products";

fn retire_former_program(runtime: &Runtime, root: &std::path::Path) -> Result<()> {
    let former = root.join(FORMER_PROGRAM);
    if !former.is_dir() {
        return Ok(());
    }
    let retired = runtime.home.join(".local/state/stado/retired");
    let destination = retired.join(FORMER_PROGRAM);
    if destination.exists() {
        bail!(
            "{} is still in the receipt tree and {} already exists; merge or remove one of them",
            former.display(),
            destination.display()
        );
    }
    fs::create_dir_all(&retired)?;
    fs::rename(&former, &destination).with_context(|| {
        format!(
            "moving the former program's state {} to {}",
            former.display(),
            destination.display()
        )
    })
}

pub fn all(runtime: &Runtime) -> Result<Vec<ProductState>> {
    let root = runtime.home.join(".stado/products");
    if !root.exists() {
        return Ok(Vec::new());
    }
    retire_former_program(runtime, &root)?;
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
