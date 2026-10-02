//! Both verification and delivery consume the schema build's artifact, never
//! whatever migrations happen to remain in the worker's current directory.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Result};

use super::super::python::{find, safe_unpack};
use super::super::{output_dir, required};
use super::migration_files;

const BUNDLE: &str = "database-schema.tar";

fn unpack(bundle: &Path, migrations: &str, work: &Path) -> Result<Vec<PathBuf>> {
    let relative = Path::new(migrations);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        bail!("--migrations must name a directory inside the schema bundle, not {migrations:?}");
    }
    let source = work.join("bundle");
    fs::create_dir_all(&source)?;
    safe_unpack(bundle, &source)?;
    migration_files(&source.join("source").join(relative))
}

pub(super) fn built(migrations: &str) -> Result<Vec<PathBuf>> {
    let output = output_dir()?;
    let work = output.join(format!("schema-input-{}", uuid::Uuid::new_v4()));
    unpack(&output.join("release").join(BUNDLE), migrations, &work)
}

pub(super) fn delivered(migrations: &str) -> Result<Vec<PathBuf>> {
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let digest = required("WISENT_RELEASE_SHA256")?;
    let actual = crate::common::sha256(&archive)?;
    if actual != digest {
        bail!(
            "the release archive {} is {actual}, not the published {digest}; no schema was applied",
            archive.display()
        );
    }
    let work = output_dir()?.join(format!("schema-delivery-{}", uuid::Uuid::new_v4()));
    let release = work.join("release");
    fs::create_dir_all(&release)?;
    safe_unpack(&archive, &release)?;
    let bundles = find(&release, BUNDLE)?;
    let bundle = match bundles.as_slice() {
        [bundle] => bundle,
        found => bail!(
            "the release holds {} {BUNDLE} (one is required); no schema was applied",
            found.len()
        ),
    };
    unpack(bundle, migrations, &work)
}
