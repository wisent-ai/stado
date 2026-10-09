//! `stado product schema verify --engine supabase`: every migration of the staged bundle
//! applied to a scratch local database that no other run shares.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use stado_wait as wait;

use super::super::output_dir;
use super::super::python::safe_unpack;
use super::{bundle_project_dir, BUNDLE};

/// Run the Supabase CLI against the local stack, answering its combined
/// output; a failure carries that output, which names the migration file and
/// statement Postgres refused.
fn local(source: &Path, arguments: &[&str]) -> Result<()> {
    let output = wait::output(
        Command::new("supabase")
            .args(arguments)
            .current_dir(source)
            .stdin(Stdio::null()),
    )
    .with_context(|| {
        format!(
            "cannot run supabase {} (the runner needs the Supabase CLI and Docker)",
            arguments.join(" ")
        )
    })?;
    if !output.status.success() {
        bail!(
            "supabase {} failed with {}: {}{}",
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Give the unpacked scratch copy its own `project_id` and its own host ports
/// and answer the id. The local stack names its containers and volumes after
/// the id and binds every `*port` key of `config.toml` on the host (a key the
/// config omits takes the CLI's fixed default, `db.port` 54322 among them),
/// so two verifications on one worker, or a developer's own local stack of
/// any product, would otherwise share a database or refuse to start on a
/// taken port. Every declared integer `*port` gets a free port, and
/// `db.port`, the one `supabase db start` binds, is set even when omitted. A
/// config without a string `project_id` is refused: the CLI would fall back
/// to a shared default name, and the cleanup would stop a database this run
/// never started.
fn isolate(source: &Path) -> Result<String> {
    let config = source.join("supabase/config.toml");
    let text = fs::read_to_string(&config)
        .with_context(|| format!("the bundle holds no {}", config.display()))?;
    let mut table: toml::Table = text
        .parse()
        .with_context(|| format!("{} is not valid TOML", config.display()))?;
    if !matches!(table.get("project_id"), Some(toml::Value::String(_))) {
        bail!(
            "{} declares no project_id string (one is required to give the scratch database \
             its own name)",
            config.display()
        );
    }
    let scratch = format!("verify-{}", uuid::Uuid::new_v4().simple());
    table.insert("project_id".into(), toml::Value::String(scratch.clone()));
    // Held until the config is written, so every port handed out is distinct.
    let mut reserved = Vec::new();
    reassign_ports(&mut table, &mut reserved)?;
    let db = table
        .entry("db")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let Some(db) = db.as_table_mut() else {
        bail!(
            "{} declares db as something other than a table",
            config.display()
        );
    };
    if !matches!(db.get("port"), Some(toml::Value::Integer(_))) {
        db.insert(
            "port".into(),
            toml::Value::Integer(free_port(&mut reserved)?),
        );
    }
    fs::write(&config, toml::to_string(&table)?)?;
    drop(reserved);
    Ok(scratch)
}

/// Give every integer value whose key ends in `port`, at any depth, a free
/// host port.
fn reassign_ports(
    table: &mut toml::Table,
    reserved: &mut Vec<std::net::TcpListener>,
) -> Result<()> {
    for (key, value) in table.iter_mut() {
        match value {
            toml::Value::Table(inner) => reassign_ports(inner, reserved)?,
            toml::Value::Integer(port) if key.ends_with("port") => *port = free_port(reserved)?,
            _ => {}
        }
    }
    Ok(())
}

/// A port the host has free now, kept reserved by its listener in `reserved`.
fn free_port(reserved: &mut Vec<std::net::TcpListener>) -> Result<i64> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .context("cannot reserve a free host port for the scratch database")?;
    let port = listener.local_addr()?.port();
    reserved.push(listener);
    Ok(i64::from(port))
}

/// `stado product schema verify --engine supabase`: the post-build test of a supabase-source
/// platform. It unpacks the staged `release/supabase-source.tar` exactly as
/// the delivery will, starts a scratch local database from its config (the
/// Supabase CLI applies every migration in order on start, with the auth,
/// storage and extension schemas production has), and stops it again without
/// keeping a volume. A migration Postgres refuses fails the test with the
/// CLI's own report, so a schema that cannot apply never qualifies.
pub fn verify(project_dir: &str) -> Result<i32> {
    let output = output_dir()?;
    let bundle = output.join("release").join(BUNDLE);
    if !bundle.is_file() {
        bail!(
            "{} is not staged; the supabase-source build writes it",
            bundle.display()
        );
    }
    let work = output.join(format!("supabase-verify-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work)?;
    let result = (|| -> Result<()> {
        safe_unpack(&bundle, &work)?;
        let source = bundle_project_dir(&work, project_dir)?;
        // Only a database this run named can be stopped: until the scratch
        // project_id is written, nothing was started and nothing is stopped.
        let scratch = isolate(&source)?;
        let started = local(&source, &["db", "start"]);
        let stopped = local(
            &source,
            &["stop", "--no-backup", "--project-id", scratch.as_str()],
        );
        started?;
        stopped
    })();
    let removed = fs::remove_dir_all(&work)
        .with_context(|| format!("cannot remove the scratch copy {}", work.display()));
    result?;
    removed?;
    println!("every migration in {BUNDLE} applied to a scratch database");
    Ok(0)
}
