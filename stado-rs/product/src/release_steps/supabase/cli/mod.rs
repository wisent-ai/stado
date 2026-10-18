//! The Supabase CLI every Supabase schema step Stado runs uses: the release
//! `stado-rs/data/declarations/supabase-cli.json` declares, from the CLI's own
//! GitHub releases.
//!
//! The post-build test of a schema release starts a scratch database with the
//! CLI and the delivery pushes with it. A Linux builder carries no Supabase
//! CLI, so without one installed the test would fail with `cannot run
//! supabase db start: No such file or directory` before a migration is read.
//! A host installs the declared
//! release under `~/.local/share/stado-supabase/<version>`, verified against
//! the release's `checksums.txt`, and every step runs that binary by its
//! path, so the CLI on the search path, of whatever version, never stands in.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::node_runtime::published;

/// The declaration's path in the Stado repository, for messages.
const DECLARATION_PATH: &str = "stado-rs/data/declarations/supabase-cli.json";
const DECLARATION: &str = include_str!("../../../../../data/declarations/supabase-cli.json");
const SCHEMA: &str = "stado.supabase-cli.v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    schema: String,
    /// The exact release every host runs, as `supabase --version` answers it.
    version: String,
    /// Where the release's assets are published, one directory per tag.
    distribution: String,
    /// Why this release.
    #[allow(dead_code)]
    source: String,
}

fn declaration() -> Result<Declaration> {
    let declaration: Declaration = serde_json::from_str(DECLARATION)
        .with_context(|| format!("{DECLARATION_PATH} is not a Supabase CLI declaration"))?;
    if declaration.schema != SCHEMA {
        bail!(
            "{DECLARATION_PATH} declares schema {}; this Stado reads {SCHEMA}",
            declaration.schema
        );
    }
    Ok(declaration)
}

fn binary(home: &Path, declaration: &Declaration) -> PathBuf {
    root(home).join(&declaration.version).join("supabase")
}

fn root(home: &Path) -> PathBuf {
    home.join(".local/share/stado-supabase")
}

/// The declared release's binary when it is installed and answers with its
/// version ([`published::answering`]).
fn installed(home: &Path, declaration: &Declaration) -> Result<Option<PathBuf>> {
    published::answering(binary(home, declaration), &declaration.version)
}

/// The declared Supabase CLI on this host, installed first when it is absent
/// or another release. A host whose install fails says which step failed.
pub(super) fn ensure() -> Result<PathBuf> {
    let home = PathBuf::from(
        std::env::var_os("HOME")
            .context("HOME is not set; the Supabase CLI is installed under it")?,
    );
    let declaration = declaration()?;
    if let Some(binary) = installed(&home, &declaration)? {
        return Ok(binary);
    }
    let version = &declaration.version;
    // The releases name a build `<os>_<arch>`, the spelling of this
    // machine's platform with an underscore.
    let platform = crate::common::platform()?.replace('-', "_");
    let base = format!("{}/v{version}", declaration.distribution);
    let archive_name = format!("supabase_{version}_{platform}.tar.gz");
    published::install(
        &published::Published {
            archive_url: format!("{base}/{archive_name}"),
            sums_url: format!("{base}/checksums.txt"),
            tree: None,
            archive_name,
        },
        &root(&home),
        version,
    )
    .with_context(|| {
        format!("installing the Supabase CLI {version} {DECLARATION_PATH} declares")
    })?;
    installed(&home, &declaration)?.with_context(|| {
        format!(
            "{} was installed but does not answer {version} to --version",
            binary(&home, &declaration).display()
        )
    })
}
