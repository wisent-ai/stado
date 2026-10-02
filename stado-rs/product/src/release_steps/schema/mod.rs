//! `stado product schema verify` and `stado product deliver schema`: the
//! release steps of a database-schema product, whatever engine it runs on.
//! The operation is the product's — apply every migration of the release to
//! a scratch database and prove it, then apply it to the real one — and the
//! engine is an adapter behind it: SQLite through the bundled driver,
//! Postgres through `psql`, Supabase through the Supabase CLI and its own
//! migration history. An engine this program has no adapter for is refused
//! by name with the ones it has.

mod postgres;
mod sqlite;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::output_dir;

/// The engines a schema product can release on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaEngine {
    Sqlite,
    Postgres,
    Supabase,
}

impl SchemaEngine {
    pub const NAMES: [&'static str; 3] = ["sqlite", "postgres", "supabase"];

    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "sqlite" => Ok(Self::Sqlite),
            "postgres" => Ok(Self::Postgres),
            "supabase" => Ok(Self::Supabase),
            other => bail!(
                "no schema adapter for engine {other:?}; this program releases schemas on {}",
                Self::NAMES.join(", ")
            ),
        }
    }
}

/// The ordered migration files of a directory: every `*.sql`, by name, so
/// the order on disk is the order applied everywhere.
pub(crate) fn migration_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(directory)
        .with_context(|| format!("reading the migrations directory {}", directory.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_some_and(|extension| extension == "sql") && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    if files.is_empty() {
        bail!("{} holds no *.sql migration; a schema release applies at least one", directory.display());
    }
    Ok(files)
}

/// The migration's version: its file name without `.sql`, the key every
/// engine's history records.
pub(crate) fn version_of(path: &Path) -> String {
    path.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `stado product schema verify --engine E [--migrations DIR] [--project-dir DIR]`.
pub fn verify(engine: &str, migrations: &str, project_dir: &str) -> Result<i32> {
    match SchemaEngine::parse(engine)? {
        SchemaEngine::Supabase => super::supabase::verify(project_dir),
        SchemaEngine::Sqlite => {
            let files = migration_files(Path::new(migrations))?;
            let scratch = output_dir()?.join("schema-verify");
            fs::create_dir_all(&scratch)?;
            sqlite::apply_all(&scratch.join("scratch.sqlite"), &files, &scratch.join("schema-verify.json"))
        }
        SchemaEngine::Postgres => {
            let files = migration_files(Path::new(migrations))?;
            let url = super::required("WISENT_SCRATCH_DATABASE_URL")?;
            postgres::verify(&url, &files, &output_dir()?.join("schema-verify.json"))
        }
    }
}

/// `stado product deliver schema --engine E [--migrations DIR] [--project-dir DIR]`.
pub fn deliver(engine: &str, migrations: &str, project_dir: &str) -> Result<i32> {
    match SchemaEngine::parse(engine)? {
        SchemaEngine::Supabase => super::supabase::deliver(project_dir),
        SchemaEngine::Sqlite => bail!(
            "a SQLite schema has no delivery: the database is a file the product opens itself, and its \
             migrations are applied by the product on open"
        ),
        SchemaEngine::Postgres => {
            let files = migration_files(Path::new(migrations))?;
            let url = super::required("SCHEMA_DATABASE_URL")?;
            postgres::deliver(&url, &files, &output_dir()?.join("schema-receipt.json"))
        }
    }
}
