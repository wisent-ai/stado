use crate::native::protocol::{SETTINGS, SETTINGS_SCHEMA};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Deserialize, Serialize)]
pub struct Identifier {
    pub uri: String,
}
#[derive(Deserialize, Serialize)]
pub struct Source {
    pub uri: String,
    pub kind: u8,
    pub generated: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub id: Identifier,
    pub display_name: String,
    pub language_ids: Vec<String>,
    pub dependencies: Vec<Identifier>,
    pub compiler: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub sources: Vec<Source>,
}
#[derive(Deserialize)]
struct Settings {
    schema_version: u32,
    package_path: PathBuf,
    targets: Vec<Target>,
}

#[derive(Default)]
pub struct Cache {
    signature: Option<(u64, SystemTime, u64)>,
    targets: BTreeMap<String, Target>,
}
impl Cache {
    pub fn invalidate(&mut self) {
        self.signature = None;
    }
    pub fn load(&mut self, package: &Path) -> Result<&BTreeMap<String, Target>> {
        let path = package.join(SETTINGS);
        let result = (|| -> Result<()> {
            let metadata = fs::metadata(&path)?;
            #[cfg(unix)]
            let inode = metadata.ino();
            #[cfg(not(unix))]
            let inode = 0;
            let signature = (inode, metadata.modified()?, metadata.len());
            if self.signature == Some(signature) {
                return Ok(());
            }
            let settings: Settings = serde_json::from_slice(&fs::read(&path)?)?;
            if settings.schema_version != SETTINGS_SCHEMA || settings.package_path != package {
                bail!("compiler settings have an unsupported format or belong to another package");
            }
            if settings.targets.is_empty() {
                bail!("compiler settings contain no targets");
            }
            let mut targets = BTreeMap::new();
            for target in settings.targets {
                if targets.insert(target.id.uri.clone(), target).is_some() {
                    bail!("compiler settings contain a repeated target identifier");
                }
            }
            self.targets = targets;
            self.signature = Some(signature);
            Ok(())
        })();
        if let Err(error) = result {
            self.invalidate();
            return Err(error).with_context(|| format!("read native compiler settings {}. Run stado product swift --package-path {} index. No dependency resolution or source clone was attempted", path.display(), package.display()));
        }
        Ok(&self.targets)
    }
}
