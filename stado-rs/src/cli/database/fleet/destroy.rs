//! `stado database destroy`: the inverse of `create --provider fleet`.
//!
//! `remove` withdraws a declaration and nothing else, which is right for a
//! hosted database Stado does not run. A fleet database is three things
//! `create` made: the managed unit `<name>-database` that serves postgres, the
//! credential item named by the declaration in the owner vault, and the
//! declaration itself. `destroy` takes them back in that order and withdraws
//! the declaration last, so a run that stops part-way still finds the
//! database declared and a second run picks up where the first stopped. The
//! data directory under `~/.stado/databases/<name>/` on the placed host is
//! left in place and named in the answer: deleting a database's data is not
//! something a typo should do.

use serde_json::{json, Value};

use crate::cli::CmdError;

use super::super::writes::mutate_databases;

pub(in crate::cli::database) async fn destroy(
    name: &str,
    host: Option<&str>,
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

    if engine == "postgres" {
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

    let data = format!("~/.stado/databases/{name}/ on {host}");
    let outcome = json!({
        "destroyed": name,
        "engine": engine,
        "host": host,
        "steps": steps,
        "data_left_in_place": data,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        println!(
            "destroyed {name}: unit, item {item} and declaration removed; its data stays at {data}"
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
