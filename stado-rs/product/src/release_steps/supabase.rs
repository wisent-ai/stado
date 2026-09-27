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

pub fn deliver() -> Result<i32> {
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
        let source = unpacked.join("source");
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
