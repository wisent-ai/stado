//! The Supabase Auth server (GoTrue) a fleet database's accounts are served
//! by: the release `stado-rs/data/declarations/supabase-auth.json` declares,
//! from its publisher's GitHub releases.
//!
//! A hosted Supabase project serves `/auth/v1` with this server over the
//! project's own `auth` schema. A database moved to the fleet keeps that
//! schema (`stado database migrate` restores it), and `stado database auth
//! place` serves it with the same server, so its users, sessions and tokens
//! keep working without the vendor. A host installs the declared release
//! under `~/.local/share/stado-supabase-auth/<version>`, verified against the
//! digest the declaration pins for this platform (the publisher lists none
//! beside its archives), and the unit runs that binary by its path, so no
//! other copy stands in.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::node_runtime::published;

/// The declaration's path in the Stado repository, for messages.
pub const DECLARATION_PATH: &str = "stado-rs/data/declarations/supabase-auth.json";
const DECLARATION: &str = include_str!("../../data/declarations/supabase-auth.json");
const SCHEMA: &str = "stado.supabase-auth.v1";
/// What the server answers its version to: `auth version` prints the tag.
const VERSION_ARGUMENTS: &[&str] = &["version"];

/// One archive of the release for one platform.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Archive {
    name: String,
    sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    schema: String,
    /// The exact release every host runs, as `auth version` answers it.
    version: String,
    /// Where the release's assets are published, one directory per tag.
    distribution: String,
    /// The archive of each platform, as `crate::common::platform` names it.
    archives: std::collections::BTreeMap<String, Archive>,
    /// Why this release.
    #[allow(dead_code)]
    source: String,
}

fn declaration() -> Result<Declaration> {
    let declaration: Declaration = serde_json::from_str(DECLARATION)
        .with_context(|| format!("{DECLARATION_PATH} is not a Supabase Auth declaration"))?;
    if declaration.schema != SCHEMA {
        bail!(
            "{DECLARATION_PATH} declares schema {}; this Stado reads {SCHEMA}",
            declaration.schema
        );
    }
    Ok(declaration)
}

fn root(home: &Path) -> PathBuf {
    home.join(".local/share/stado-supabase-auth")
}

fn binary(home: &Path, declaration: &Declaration) -> PathBuf {
    root(home).join(&declaration.version).join("auth")
}

/// The declared release's server when it is installed and answers with its
/// version ([`published::answering`]).
fn installed(home: &Path, declaration: &Declaration) -> Result<Option<PathBuf>> {
    published::answering(binary(home, declaration), VERSION_ARGUMENTS, &declaration.version)
}

/// The declared Supabase Auth server on this host under `home`, with the
/// release it is: installed first when it is absent or another release. A
/// platform the declaration names no archive for, a download, a digest that
/// is not the pinned one, or a server that does not answer its release each
/// refuse naming the step.
pub fn ensure(home: &Path) -> Result<(PathBuf, String)> {
    let declaration = declaration()?;
    let version = declaration.version.clone();
    if let Some(server) = installed(home, &declaration)? {
        return Ok((server, version));
    }
    let platform = crate::common::platform()?;
    let archive = declaration.archives.get(&platform).with_context(|| {
        format!(
            "{DECLARATION_PATH} declares no Supabase Auth archive for {platform}; it declares {}",
            declaration.archives.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    published::install(
        &published::Published {
            archive_url: format!("{}/{version}/{}", declaration.distribution, archive.name),
            expected: published::Expected::Pinned(archive.sha256.clone()),
            tree: None,
            archive_name: archive.name.clone(),
        },
        &root(home),
        &version,
    )
    .with_context(|| format!("installing Supabase Auth {version} {DECLARATION_PATH} declares"))?;
    let server = installed(home, &declaration)?.with_context(|| {
        format!(
            "{} was installed but does not answer {version} to `{}`",
            binary(home, &declaration).display(),
            VERSION_ARGUMENTS.join(" ")
        )
    })?;
    Ok((server, version))
}
