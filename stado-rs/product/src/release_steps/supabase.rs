//! `stado product deliver supabase`: a Supabase project's migrations and
//! functions, from the verified release, pushed to the project the delivery's
//! secrets name. The same Python file was copied into wisent-supabase-oko,
//! -preferences and -wisent-app; the oko copy also carried a split migration's
//! later parts in as applied, which this does for any project that declares
//! `supabase/split-migrations.json`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::python::{find, safe_unpack};
use super::{output_dir, required, RECORD_SCHEMA};

/// The bundle a Supabase product's build stages.
const BUNDLE: &str = "supabase-source.tar";

/// Run the Supabase CLI in `source` and answer its standard output.
fn supabase(source: &Path, arguments: &[&str], token: &str, password: &str) -> Result<String> {
    let output = Command::new("supabase")
        .args(arguments)
        .current_dir(source)
        .env("SUPABASE_ACCESS_TOKEN", token)
        .env("SUPABASE_DB_PASSWORD", password)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot run supabase {}", arguments.join(" ")))?;
    if !output.status.success() {
        bail!(
            "supabase {} failed with {}: {}",
            arguments.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Run the Supabase CLI against the local stack, answering its combined
/// output; a failure carries that output, which names the migration file and
/// statement Postgres refused.
fn local(source: &Path, arguments: &[&str]) -> Result<()> {
    let output = Command::new("supabase")
        .args(arguments)
        .current_dir(source)
        .stdin(Stdio::null())
        .output()
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
/// the id and binds every `*port` key of `config.toml` on the host, so two
/// verifications on one worker, or a developer's own local stack of any
/// product, would otherwise share a database or refuse to start on a taken
/// port. A config without exactly one `project_id` line is refused: the CLI
/// would fall back to a shared default name, and the cleanup would stop a
/// database this run never started.
fn isolate(source: &Path) -> Result<String> {
    let config = source.join("supabase/config.toml");
    let text = fs::read_to_string(&config)
        .with_context(|| format!("the bundle holds no {}", config.display()))?;
    let scratch = format!("verify-{}", uuid::Uuid::new_v4().simple());
    let mut replaced = 0;
    // Held until the config is written, so every port handed out is distinct.
    let mut reserved = Vec::new();
    let mut lines = Vec::new();
    for line in text.lines() {
        let (key, value) = line.split_once('=').unwrap_or((line, ""));
        let key = key.trim();
        if key == "project_id" {
            replaced += 1;
            lines.push(format!("project_id = \"{scratch}\""));
        } else if key.ends_with("port") && value.trim().parse::<u16>().is_ok() {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")
                .context("cannot reserve a free host port for the scratch database")?;
            lines.push(format!("{key} = {}", listener.local_addr()?.port()));
            reserved.push(listener);
        } else {
            lines.push(line.to_owned());
        }
    }
    if replaced != 1 {
        bail!(
            "{} declares {replaced} project_id lines (one is required to give the scratch \
             database its own name)",
            config.display()
        );
    }
    fs::write(&config, lines.join("\n") + "\n")?;
    drop(reserved);
    Ok(scratch)
}

/// The Supabase project directory inside an unpacked bundle: `project_dir`
/// relative to its `source/` root, refused when it would leave that root.
fn bundle_project_dir(unpacked: &Path, project_dir: &str) -> Result<PathBuf> {
    let relative = Path::new(project_dir);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        bail!("--project-dir {project_dir} must be a relative path inside the bundle");
    }
    Ok(unpacked.join("source").join(relative))
}

/// `stado product supabase verify`: the post-build test of a supabase-source
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

pub fn deliver(project_dir: &str) -> Result<i32> {
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let digest = required("WISENT_RELEASE_SHA256")?;
    if crate::common::sha256(&archive)? != digest {
        bail!(
            "the release archive {} is not the published {digest}; nothing was pushed",
            archive.display()
        );
    }
    let token = required("SUPABASE_ACCESS_TOKEN")?;
    let password = required("SUPABASE_DB_PASSWORD")?;
    let project = required("SUPABASE_PROJECT_REF")?;
    let work = output_dir()?.join(format!("supabase-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&work)?;
    let result = (|| -> Result<Value> {
        let release = work.join("release");
        fs::create_dir_all(&release)?;
        safe_unpack(&archive, &release)?;
        let bundle = match find(&release, BUNDLE)?.as_slice() {
            [bundle] => bundle.clone(),
            found => bail!(
                "the release holds {} {BUNDLE} (one is required)",
                found.len()
            ),
        };
        let unpacked = work.join("bundle");
        fs::create_dir_all(&unpacked)?;
        safe_unpack(&bundle, &unpacked)?;
        let source = bundle_project_dir(&unpacked, project_dir)?;
        supabase(
            &source,
            &["link", "--project-ref", &project],
            &token,
            &password,
        )?;
        // A migration the database already applied may have been split into
        // parts; the later parts carry SQL that database already ran, so they
        // are recorded as applied there instead of run again.
        let split = source.join("supabase/split-migrations.json");
        let mut carried = Vec::new();
        let mut repaired = Value::Null;
        if split.is_file() {
            let listed = supabase(
                &source,
                &["migration", "list", "--linked"],
                &token,
                &password,
            )?;
            let applied: BTreeSet<String> = listed
                .lines()
                .filter_map(|line| line.split('|').nth(1).map(str::trim))
                .filter(|cell| !cell.is_empty() && cell.chars().all(|c| c.is_ascii_digit()))
                .map(str::to_owned)
                .collect();
            let parts: Value = serde_json::from_slice(&fs::read(&split)?)?;
            for (part, original) in parts["parts"].as_object().into_iter().flatten() {
                if original
                    .as_str()
                    .is_some_and(|original| applied.contains(original))
                    && !applied.contains(part)
                {
                    carried.push(part.clone());
                }
            }
            carried.sort();
            if !carried.is_empty() {
                let mut arguments = vec!["migration", "repair", "--status", "applied"];
                arguments.extend(carried.iter().map(String::as_str));
                repaired = json!(supabase(&source, &arguments, &token, &password)?);
            }
        }
        let database = supabase(&source, &["db", "push", "--include-all"], &token, &password)?;
        let functions = if source.join("supabase/functions").is_dir() {
            json!(supabase(
                &source,
                &["functions", "deploy", "--project-ref", &project],
                &token,
                &password
            )?)
        } else {
            Value::Null
        };
        Ok(
            json!({"database": database, "functions": functions, "carried_parts": carried, "repaired": repaired}),
        )
    })();
    let _ = fs::remove_dir_all(&work);
    let pushed = result?;
    let receipt = json!({
        "schema_version": RECORD_SCHEMA, "channel": "supabase",
        "product": required("WISENT_PRODUCT")?, "version": required("WISENT_VERSION")?,
        "release_uri": required("WISENT_RELEASE_URI")?, "release_sha256": digest,
        "project_ref": project, "database": pushed["database"], "functions": pushed["functions"],
        "carried_parts": pushed["carried_parts"], "repaired": pushed["repaired"],
    });
    fs::write(
        output_dir()?.join("supabase-receipt.json"),
        format!("{receipt}\n"),
    )?;
    println!("pushed the release's migrations to Supabase project {project}");
    Ok(0)
}
