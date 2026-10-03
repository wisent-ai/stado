//! `stado database destroy`: the inverse of `create`, for every provider
//! Stado creates with.
//!
//! `remove` withdraws a declaration and nothing else, which is right for an
//! external database Stado does not run, so `destroy` refuses one. A fleet
//! database is three things `create` made: the managed unit `<name>-database`
//! that serves postgres, the credential item named by the declaration in the
//! owner vault, and the declaration itself. A Supabase database is the hosted
//! project, the item and the declaration; deleting the project deletes its
//! data, so it needs `--delete-project`. `destroy` takes them back in that
//! order and withdraws the declaration last, so a run that stops part-way
//! still finds the database declared and a second run picks up where the
//! first stopped. A fleet database's data directory under
//! `~/.stado/databases/<name>/` on the placed host is left in place and
//! named in the answer: deleting a database's data is not something a typo
//! should do.

use serde_json::{json, Value};

use crate::cli::CmdError;

use super::super::writes::mutate_databases;

pub(in crate::cli::database) async fn destroy(
    name: &str,
    host: Option<&str>,
    delete_project: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let databases = crate::config::database_api_databases()
        .map_err(|problems| CmdError::click(problems.join("; ")))?;
    let declared = databases.get(name).ok_or_else(|| {
        CmdError::usage(format!(
            "unknown database {name:?}; declared: {}",
            databases.keys().cloned().collect::<Vec<_>>().join(", ")
        ))
    })?;
    let engine = declared.engine().to_string();
    let item = declared.item().to_string();
    let (owner, here) = crate::cli::release_catalog::fleet_hosts().await?;
    let host = host.map(str::to_string).unwrap_or_else(|| owner.clone());
    let mut steps: Vec<Value> = Vec::new();
    let provider = provider_of(&item, owner == here).await?;
    if provider == "external" {
        return Err(CmdError::usage(format!(
            "{name} is an external {engine} server Stado does not run; `stado database remove {name}` \
             withdraws its declaration and leaves the server and its credential item untouched"
        )));
    }
    if provider == "supabase" {
        if !delete_project {
            return Err(CmdError::usage(format!(
                "{name} is a Supabase project; destroying it deletes the project and every row in it, \
                 which cannot be restored; add --delete-project, or `stado database remove {name}` to keep it"
            )));
        }
        let (reference, status) = crate::cli::database::supabase::destroy::delete_project(&item)
            .await
            .map_err(|error| {
                partial(
                    name,
                    &steps,
                    format!("the Supabase project was not deleted: {error}"),
                )
            })?;
        steps.push(json!({"step": "project", "project_ref": reference, "status": status}));
    }

    if provider == "fleet" && engine == "postgres" {
        let unit = format!("{name}-database");
        let target = crate::deploy::host_channel::canonical_target(&host)
            .await
            .map_err(|error| CmdError::click(format!("{host}: {error}")))?;
        let still_declared = crate::deploy::service::declared_services(&target)
            .iter()
            .any(|service| service.matches(&unit));
        if still_declared {
            if let Err(error) =
                crate::cli::service::lifecycle::adopt::removal::remove(&unit, &host, false).await
            {
                return Err(partial(
                    name,
                    &steps,
                    format!("the unit {unit} on {host} was not removed: {error}"),
                ));
            }
        }
        steps.push(json!({
            "step": "unit",
            "unit": unit,
            "host": host,
            "status": if still_declared { "removed" } else { "already absent" },
        }));
    }

    let deleted = if owner == here {
        match crate::credential_store::owner::item_exists(&item) {
            Ok(false) => Ok("already absent"),
            Ok(true) => crate::credential_store::owner::delete_item(&item)
                .map(|()| "deleted")
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        }
    } else {
        crate::cli::host::delete_vault_item(&owner, &item, false)
            .await
            .map(|()| "deleted")
            .map_err(|error| error.to_string())
    };
    let status = match deleted {
        Ok(status) => status,
        Err(error) => {
            return Err(partial(
                name,
                &steps,
                format!("the item {item} is still in {owner}'s vault: {error}"),
            ));
        }
    };
    steps.push(json!({"step": "item", "item": item, "vault": owner, "status": status}));

    mutate_databases(|map| {
        map.remove(name);
        Ok(())
    })
    .map_err(|error| {
        partial(
            name,
            &steps,
            format!("the declaration was not withdrawn: {error}"),
        )
    })?;
    steps.push(json!({"step": "declaration", "status": "removed"}));

    let data = if provider == "supabase" {
        String::from("deleted with the Supabase project")
    } else {
        format!("left in place at ~/.stado/databases/{name}/ on {host}")
    };
    let outcome = json!({
        "destroyed": name,
        "provider": provider,
        "engine": engine,
        "host": host,
        "steps": steps,
        "data": data,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        let parts: Vec<&str> = steps
            .iter()
            .filter_map(|step| step["step"].as_str())
            .collect();
        println!(
            "destroyed {name} ({provider}): {} removed; its data is {data}",
            parts.join(", ")
        );
    }
    Ok(())
}

/// A refusal that says which parts are already gone; the declaration stays
/// until the last step, so running `destroy` again continues from here.
fn partial(name: &str, done: &[Value], reason: String) -> CmdError {
    let finished: Vec<&str> = done
        .iter()
        .filter_map(|step| step["step"].as_str())
        .collect();
    CmdError::click(format!(
        "{name} is not destroyed{}: {reason}; run destroy again once that is repaired",
        if finished.is_empty() {
            String::new()
        } else {
            format!(" ({} already removed)", finished.join(", "))
        }
    ))
}

/// The provider the credential item records: `provider` on fleet and
/// external items, a `project_ref` on a Supabase project's. An item that
/// holds neither was already deleted by an earlier run, whose provider steps
/// ran before it; that run is continued with the item and declaration steps.
async fn provider_of(item: &str, local_owner: bool) -> Result<String, CmdError> {
    // Creation and deletion already use owner authority on this host. Reading
    // their metadata must not require an unrelated workload bearer.
    let document = if local_owner {
        crate::credential_store::owner::read_document(item).map_err(|error| {
            CmdError::click(format!(
                "{item} could not be read from the owner vault: {error}"
            ))
        })?
    } else {
        None
    };
    let fields = document
        .as_ref()
        .and_then(|document| document.get("fields"));
    let read = |field: &'static str| async move {
        let value = if local_owner {
            crate::skarbiec::envelope::plain(
                fields
                    .and_then(|fields| fields.get(field))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        } else {
            crate::credential_store::read_declared_string(item, field).await
        };
        value.map_err(|error| CmdError::click(format!("{item}.{field} could not be read: {error}")))
    };
    if let Some(provider) = read("provider").await? {
        return Ok(provider);
    }
    if read("project_ref").await?.is_some() {
        return Ok(String::from("supabase"));
    }
    Ok(String::from("already destroyed"))
}
