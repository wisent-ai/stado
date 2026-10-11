//! `stado database adopt [NAME] [--project-ref REF]`: bring a declared
//! database's credential item in line with its hosted Supabase project.
//!
//! The item `<name>-database` is rewritten from the management API — the
//! project's coordinates, its pooler, its revealed API keys (the legacy
//! `anon` and `service_role`, and every named publishable or secret key as
//! `publishable_key_<name>` / `secret_key_<name>`), and its active custom
//! hostname as `custom_url` — so a rotated key lands on the next run. Fields
//! another owner put on the item stay; the password is kept from the item,
//! taken from `--password-file`, or, with `--rotate-password`, replaced on
//! the project through the management API (`PATCH
//! /v1/projects/{ref}/database/password`) and stored on the item, for a
//! project whose password no item holds. Without NAME every declared
//! database whose item names a Supabase `project_ref` is adopted again. The
//! item is read whole in the owner vault: here on the owner host, and on any
//! other host through the host channel, where the owner's own Skarbiec reads
//! it and the rewrite goes back the same way. `--check` writes nothing and
//! exits non-zero on drift.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;

use super::owner_vault::{self, Owner};
use super::{call, token};

mod project;
use project::{project_fields, rotated_password};

/// Where the item's `db_password` comes from.
pub(in crate::cli::database) enum Password {
    /// The one the item already holds, when it holds one.
    Kept,
    /// The whole content of this file.
    File(String),
    /// A new one, set on the project before the item is written.
    Rotate,
}
/// Adopt one database; its report row, and whether its item drifted.
/// `project_ref` and `password` come from the command line, when given; the
/// item's own `project_ref` and `db_password` stand otherwise.
#[allow(clippy::too_many_arguments)]
async fn adopt_one(
    owner: &Owner,
    name: &str,
    item: &str,
    project_ref: Option<&str>,
    password: Option<String>,
    rotate: bool,
    projects: &[Value],
    token: &str,
    check: bool,
) -> Result<(Value, bool), CmdError> {
    let document = owner.document(item).await?.unwrap_or_else(|| json!({}));
    let existing = document["fields"].as_object().cloned().unwrap_or_default();
    let mut context = document["context"].as_object().cloned().unwrap_or_default();
    let supabase_item = context
        .get("provider")
        .is_none_or(|provider| provider == "supabase");
    let recorded = existing
        .get("project_ref")
        .and_then(Value::as_str)
        .filter(|_| supabase_item);
    let Some(reference) = project_ref.or(recorded).map(str::to_string) else {
        let row = json!({"database": name, "item": item, "status": "not a Supabase project item"});
        return Ok((row, false));
    };
    let project = projects
        .iter()
        .find(|project| project["ref"] == reference.as_str())
        .ok_or_else(|| {
            CmdError::click(format!(
                "project {reference} is not in the organization the access token can see"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let password = if rotate {
        owner.ready()?;
        Some(rotated_password(&reference, token).await?)
    } else {
        password.or_else(|| {
            existing
                .get("db_password")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
    };
    let mut merged = existing.clone();
    merged.extend(project_fields(project, token, password.as_deref()).await?);
    let drifted = merged != existing;
    let status = match (drifted, check) {
        (false, _) => "up to date",
        (true, true) => "would write",
        (true, false) => {
            context.entry("engine").or_insert(json!("postgres"));
            context.insert("provider".into(), json!("supabase"));
            context.entry("product").or_insert(json!(name));
            let fields = Value::Object(merged.clone());
            owner
                .store(item, "bundle", &fields, &Value::Object(context))
                .await
                .map_err(|error| {
                    if !rotate {
                        return error;
                    }
                    let mut wrapped = CmdError::click(format!(
                        "{item} was not written after the password of {reference} was rotated, \
                         so no item holds the new password; run stado database adopt {name} \
                         --rotate-password again: {error}"
                    ));
                    wrapped.failure = error.failure;
                    wrapped
                })?;
            "written"
        }
    };
    let row = json!({
        "database": name,
        "item": item,
        "project_ref": reference,
        "status": status,
        "fields": merged.len(),
        "password_on_item": password.is_some(),
    });
    Ok((row, drifted))
}

pub(in crate::cli::database) async fn adopt(
    name: Option<&str>,
    project_ref: Option<&str>,
    password: Password,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let names_one = project_ref.is_some() || !matches!(password, Password::Kept);
    if name.is_none() && names_one {
        return Err(CmdError::usage(
            "--project-ref, --password-file and --rotate-password adopt one database: name it",
        ));
    }
    let owner = owner_vault::locate().await?;
    let rotate = matches!(password, Password::Rotate);
    let password = match password {
        Password::File(path) => Some(
            std::fs::read_to_string(&path)
                .map_err(|error| {
                    CmdError::click(format!("--password-file {path}: {error}"))
                        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
                })?
                .trim()
                .to_string(),
        ),
        Password::Kept | Password::Rotate => None,
    };
    let declared = super::super::declared_databases()?;
    let mut targets = Vec::new();
    for (declared_name, database) in declared {
        if name.is_none_or(|name| name == declared_name.as_str()) {
            targets.push((declared_name.to_string(), database.item().to_string()));
        }
    }
    if let Some(name) = name.filter(|_| targets.is_empty()) {
        return Err(CmdError::click(format!(
            "{name} is not declared; declare it first with stado database declare {name}"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    let token = token().await?;
    let listed = call(reqwest::Method::GET, "/projects", &token, None).await?;
    let projects = listed.as_array().cloned().unwrap_or_default();
    let mut rows = Vec::new();
    let mut drift = 0;
    let present = owner.item_ids().await?;
    for (database, item) in &targets {
        if present.as_ref().is_some_and(|ids| !ids.contains(item)) {
            let status = format!("no item in the owner vault on {}", owner.name());
            rows.push(json!({"database": database, "item": item, "status": status}));
            continue;
        }
        let (row, drifted) = adopt_one(
            &owner,
            database,
            item,
            project_ref,
            password.clone(),
            rotate,
            &projects,
            &token,
            check,
        )
        .await?;
        drift += usize::from(drifted);
        rows.push(row);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            println!(
                "{}: {} ({} fields)",
                row["item"].as_str().unwrap_or_default(),
                row["status"].as_str().unwrap_or_default(),
                row["fields"]
            );
        }
    }
    if check && drift > 0 {
        return Err(CmdError::refused(format!(
            "{drift} item(s) differ from their Supabase projects"
        )));
    }
    Ok(())
}
