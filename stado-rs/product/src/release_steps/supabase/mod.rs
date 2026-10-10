//! The Supabase adapter of the schema release (`stado product schema verify
//! --engine supabase` and `deliver schema --engine supabase`): a Supabase
//! project's migrations and functions, from the verified release, pushed to
//! the project the delivery's secrets name. The same Python file was copied
//! into wisent-supabase-oko, -preferences and -wisent-app. Before the push the
//! linked project's history is reconciled with what the bundle declares (split
//! migrations, a baseline of versions a hand-built database already holds);
//! see [`history`].

mod history;
mod secrets;
mod verify;

pub use verify::verify;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use stado_wait as wait;

use super::python::{find, safe_unpack};
use super::{output_dir, required, RECORD_SCHEMA};

/// The bundle a Supabase product's build stages.
const BUNDLE: &str = "supabase-source.tar";

/// Run the Supabase CLI in `source` and answer its standard output. Without
/// a database password the CLI signs in with a temporary login role it
/// creates through the Management API, so the access token alone suffices.
fn supabase(
    source: &Path,
    arguments: &[&str],
    token: &str,
    password: Option<&str>,
) -> Result<String> {
    let mut command = Command::new("supabase");
    command
        .args(arguments)
        .current_dir(source)
        .env("SUPABASE_ACCESS_TOKEN", token)
        .env_remove("SUPABASE_DB_PASSWORD");
    if let Some(password) = password {
        command.env("SUPABASE_DB_PASSWORD", password);
    }
    let output = wait::output(command.stdin(Stdio::null()))
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

pub fn deliver(project_dir: &str) -> Result<i32> {
    let archive = PathBuf::from(required("WISENT_RELEASE_ARCHIVE")?);
    let digest = required("WISENT_RELEASE_SHA256")?;
    let product = required("WISENT_PRODUCT")?;
    let version = required("WISENT_VERSION")?;
    let release_uri = required("WISENT_RELEASE_URI")?;
    let output = output_dir()?;
    if crate::common::sha256(&archive)? != digest {
        bail!(
            "the release archive {} is not the published {digest}; nothing was pushed",
            archive.display()
        );
    }
    let token = required("SUPABASE_ACCESS_TOKEN")?;
    let password = match std::env::var("SUPABASE_DB_PASSWORD") {
        Ok(value) if value.trim().is_empty() => {
            bail!("SUPABASE_DB_PASSWORD is set but blank; provide the selected password or unset it to select Management API authentication")
        }
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("SUPABASE_DB_PASSWORD is set but is not valid Unicode; the selected password cannot be passed to the Supabase client")
        }
    };
    let password = password.as_deref();
    let project = required("SUPABASE_PROJECT_REF")?;
    let work = output.join(format!("supabase-{}", uuid::Uuid::new_v4()));
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
        // Resolve every declared value before the first remote mutation.
        let function_secrets = secrets::load(&source)?;
        let declarations = history::Declarations::load(&source)?;
        supabase(
            &source,
            &["link", "--project-ref", &project],
            &token,
            password,
        )?;
        // Reconcile the history before pushing, so db push applies only what
        // the database has not run: split parts and baseline versions it
        // already holds are recorded as applied, retired history rows as
        // reverted.
        let listing = supabase(
            &source,
            &["migration", "list", "--linked", "--output-format", "json"],
            &token,
            password,
        )?;
        let (carried, retired) = declarations.repairs(&history::applied(&listing)?);
        let mut answers = Vec::new();
        for (status, versions) in [("reverted", &retired), ("applied", &carried)] {
            if versions.is_empty() {
                continue;
            }
            let mut arguments = vec!["migration", "repair", "--status", status];
            arguments.extend(versions.iter().map(String::as_str));
            answers.push(json!(supabase(&source, &arguments, &token, password)?));
        }
        let repaired = history::receipt(&carried, &retired, answers);
        let database = supabase(&source, &["db", "push", "--include-all"], &token, password)?;
        let function_secrets = secrets::publish(&function_secrets, &project, &token)?;
        let functions = if source.join("supabase/functions").is_dir() {
            json!(supabase(
                &source,
                &["functions", "deploy", "--project-ref", &project],
                &token,
                password
            )?)
        } else {
            Value::Null
        };
        Ok(
            json!({"database": database, "functions": functions, "function_secrets": function_secrets, "carried_parts": carried, "repaired": repaired}),
        )
    })();
    let cleanup = fs::remove_dir_all(&work);
    let pushed = match (result, cleanup) {
        (Ok(pushed), Ok(())) => pushed,
        (Err(cause), Ok(())) => return Err(cause),
        (Ok(_), Err(cleanup)) => {
            bail!("schema delivery reached project {project}, but removing workspace {} failed: {cleanup}; delivery is not complete", work.display())
        }
        (Err(cause), Err(cleanup)) => {
            return Err(cause.context(format!(
                "removing schema delivery workspace {} also failed: {cleanup}",
                work.display()
            )));
        }
    };
    let receipt = json!({
        "schema_version": RECORD_SCHEMA, "channel": "supabase",
        "product": product, "version": version,
        "release_uri": release_uri, "release_sha256": digest,
        "project_ref": project, "database": pushed["database"], "functions": pushed["functions"],
        "function_secrets": pushed["function_secrets"],
        "carried_parts": pushed["carried_parts"], "repaired": pushed["repaired"],
    });
    fs::write(
        output.join("supabase-receipt.json"),
        format!("{receipt}\n"),
    )?;
    println!("pushed the release's migrations to Supabase project {project}");
    Ok(0)
}
