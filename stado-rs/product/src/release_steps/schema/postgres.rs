//! The Postgres adapter of the schema release, through `psql` on the runner
//! (the same way the Supabase adapter goes through the Supabase CLI). Verify
//! applies every migration to the scratch database the runner names; deliver
//! applies the ones the real database has not recorded yet, each in one
//! transaction that also records its version in `wisent_schema_migrations`,
//! so a second delivery of the same release applies nothing and says so.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::json;

use super::super::{required, RECORD_SCHEMA};
use super::version_of;

const HISTORY: &str = "create table if not exists wisent_schema_migrations (\
 version text primary key, applied_at timestamptz not null default now())";

/// Run one psql invocation with the SQL on its standard input and answer its
/// standard output; psql runs to completion and its own refusal is the error.
fn psql(url: &str, single_transaction: bool, sql: &str) -> Result<String> {
    let mut command = Command::new("psql");
    command.args(["--no-psqlrc", "--quiet", "--tuples-only", "--no-align", "-v", "ON_ERROR_STOP=1"]);
    if single_transaction {
        command.arg("--single-transaction");
    }
    let mut child = command
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("cannot run psql; install the PostgreSQL client on the runner")?;
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .context("psql standard input is not open")?
            .write_all(sql.as_bytes())
            .context("writing SQL to psql")?;
    }
    let output = child.wait_with_output().context("waiting for psql")?;
    if !output.status.success() {
        bail!(
            "psql exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn migration_sql(file: &Path) -> Result<String> {
    fs::read_to_string(file).with_context(|| format!("reading migration {}", file.display()))
}

/// Every migration, in order, against the scratch database; a failure names
/// the migration and carries psql's own sentence.
pub fn verify(url: &str, files: &[PathBuf], report: &Path) -> Result<i32> {
    let mut applied = Vec::new();
    for file in files {
        psql(url, true, &migration_sql(file)?)
            .with_context(|| format!("migration {} failed on Postgres", file.display()))?;
        applied.push(version_of(file));
    }
    let tables = psql(
        url,
        false,
        "select table_name from information_schema.tables where table_schema = 'public' order by 1",
    )?;
    let record = json!({
        "engine": "postgres",
        "applied": applied,
        "tables": tables.lines().map(str::to_owned).collect::<Vec<_>>(),
    });
    fs::write(report, format!("{}\n", serde_json::to_string_pretty(&record)?))
        .with_context(|| format!("writing {}", report.display()))?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(0)
}

/// The migrations the real database has not recorded, applied in order,
/// each with its version recorded in the same transaction.
pub fn deliver(url: &str, files: &[PathBuf], receipt: &Path) -> Result<i32> {
    psql(url, false, HISTORY).context("creating the migration history table")?;
    let recorded = psql(url, false, "select version from wisent_schema_migrations order by 1")?;
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
            "{}\ninsert into wisent_schema_migrations (version) values ('{}');\n",
            migration_sql(file)?,
            version.replace('\'', "''")
        );
        psql(url, true, &sql)
            .with_context(|| format!("migration {} failed on the delivered database", file.display()))?;
        applied.push(version);
    }
    let record = json!({
        "schema_version": RECORD_SCHEMA, "channel": "schema", "engine": "postgres",
        "product": required("WISENT_PRODUCT")?, "version": required("WISENT_VERSION")?,
        "release_uri": required("WISENT_RELEASE_URI")?,
        "applied": applied, "already_applied": skipped,
    });
    fs::write(receipt, format!("{}\n", serde_json::to_string_pretty(&record)?))
        .with_context(|| format!("writing {}", receipt.display()))?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(0)
}
