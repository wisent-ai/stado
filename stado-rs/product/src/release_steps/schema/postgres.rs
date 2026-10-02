//! The Postgres adapter of the schema release, through `psql` on the runner
//! (the same way the Supabase adapter goes through the Supabase CLI). Verify
//! applies every migration to the scratch database the runner names; deliver
//! applies the ones the real database has not recorded yet, each in one
//! transaction that also records its version in `wisent_schema_migrations`,
//! so a second delivery of the same release applies nothing and says so.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::json;

use super::super::{required, RECORD_SCHEMA};
use super::version_of;

const HISTORY: &str = "create table if not exists wisent_schema_migrations (\
 version text primary key, applied_at timestamptz not null default now())";
/// Execute migration files directly. `--single-transaction` requires `-f`
/// or `-c`; a pipe on stdin alone does not start a transaction in psql.
fn psql(url: &str, single_transaction: bool, file: Option<&Path>, sql: &str) -> Result<String> {
    let mut command = std::process::Command::new("psql");
    command.args([
        "--no-psqlrc",
        "--no-password",
        "--quiet",
        "--tuples-only",
        "--no-align",
        "-v",
        "ON_ERROR_STOP=1",
    ]);
    if single_transaction {
        command.arg("--single-transaction");
    }
    if let Some(file) = file {
        command.arg("--file").arg(file);
    }
    if !sql.is_empty() {
        command.args(["--command", sql]);
    }
    let output = command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .output()
        .context("cannot run psql; install the PostgreSQL client on the runner")?;
    if !output.status.success() {
        bail!(
            "psql exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Every migration, in order, against the scratch database; a failure names
/// the migration and carries psql's own sentence.
pub fn verify(url: &str, files: &[PathBuf], report: &Path) -> Result<i32> {
    let mut applied = Vec::new();
    for file in files {
        psql(url, true, Some(file), "")
            .with_context(|| format!("migration {} failed on Postgres", file.display()))?;
        applied.push(version_of(file));
    }
    let tables = psql(
        url,
        false,
        None,
        "select table_name from information_schema.tables where table_schema = 'public' order by 1",
    )?;
    let record = json!({
        "engine": "postgres",
        "applied": applied,
        "tables": tables.lines().map(str::to_owned).collect::<Vec<_>>(),
    });
    fs::write(
        report,
        format!("{}\n", serde_json::to_string_pretty(&record)?),
    )
    .with_context(|| format!("writing {}", report.display()))?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(0)
}

/// The migrations the real database has not recorded, applied in order,
/// each with its version recorded in the same transaction.
pub fn deliver(url: &str, files: &[PathBuf], receipt: &Path) -> Result<i32> {
    let product = required("WISENT_PRODUCT")?;
    let version = required("WISENT_VERSION")?;
    let release_uri = required("WISENT_RELEASE_URI")?;
    let release_sha256 = required("WISENT_RELEASE_SHA256")?;
    psql(url, false, None, HISTORY).context("creating the migration history table")?;
    let recorded = psql(
        url,
        false,
        None,
        "select version from wisent_schema_migrations order by 1",
    )?;
    let recorded: Vec<&str> = recorded.lines().collect();
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for file in files {
        let version = version_of(file);
        if recorded.iter().any(|known| *known == version) {
            skipped.push(version);
            continue;
        }
        let sql = format!(
            "insert into wisent_schema_migrations (version) values ('{}');",
            version.replace('\'', "''")
        );
        psql(url, true, Some(file), &sql).with_context(|| {
            format!(
                "migration {} failed on the delivered database",
                file.display()
            )
        })?;
        applied.push(version);
    }
    let record = json!({
        "schema_version": RECORD_SCHEMA, "channel": "schema", "engine": "postgres",
        "product": product, "version": version,
        "release_uri": release_uri, "release_sha256": release_sha256,
        "applied": applied, "already_applied": skipped,
    });
    fs::write(
        receipt,
        format!("{}\n", serde_json::to_string_pretty(&record)?),
    )
    .with_context(|| format!("writing {}", receipt.display()))?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(0)
}
