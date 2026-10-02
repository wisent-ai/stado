//! The SQLite adapter of the schema release: every migration applied, in
//! order, to a fresh scratch file through the bundled driver, each in its
//! own transaction, so a migration that fails names itself and leaves the
//! ones before it visible for diagnosis.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use super::version_of;

pub fn apply_all(scratch: &Path, files: &[std::path::PathBuf], report: &Path) -> Result<i32> {
    if scratch.exists() {
        fs::remove_file(scratch).with_context(|| {
            format!(
                "removing the previous scratch database {}",
                scratch.display()
            )
        })?;
    }
    let connection = rusqlite::Connection::open(scratch)
        .with_context(|| format!("opening the scratch SQLite database {}", scratch.display()))?;
    let mut applied = Vec::new();
    for file in files {
        let sql = fs::read_to_string(file)
            .with_context(|| format!("reading migration {}", file.display()))?;
        let transaction = connection
            .unchecked_transaction()
            .with_context(|| format!("starting the transaction of migration {}", file.display()))?;
        transaction
            .execute_batch(&sql)
            .with_context(|| format!("migration {} failed on SQLite", file.display()))?;
        transaction
            .commit()
            .with_context(|| format!("committing migration {}", file.display()))?;
        applied.push(version_of(file));
    }
    let tables: Vec<String> = connection
        .prepare("select name from sqlite_master where type = 'table' order by name")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    let record = json!({
        "engine": "sqlite",
        "scratch": scratch,
        "applied": applied,
        "tables": tables,
    });
    fs::write(
        report,
        format!("{}\n", serde_json::to_string_pretty(&record)?),
    )
    .with_context(|| format!("writing {}", report.display()))?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(0)
}
