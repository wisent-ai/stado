//! `stado profiles [NAME] [--json]`: list visible profiles with their
//! one-line descriptions, or show one profile; `key: value` text by default,
//! JSON with `--json`.

use serde_json::{json, Value};

use crate::profiles;

use crate::cli::CmdError;

pub fn run(name: Option<&str>, as_json: bool) -> Result<(), CmdError> {
    if let Some(name) = name {
        // The profile's own sentence is kept as the message; its class is the
        // one the shared conversion states: an absent profile not_found, an
        // invalid one config.
        let profile = profiles::load_profile(name).map_err(|exc| match exc {
            profiles::ProfileError::NotFound(_) => CmdError::missing(exc.to_string()),
            profiles::ProfileError::Invalid(_) => CmdError::declaration(exc.to_string()),
            other => CmdError::from(other),
        })?;
        return crate::cli::print_answer(&Value::Object(profile), as_json);
    }
    let rows: Vec<Value> = profiles::list_profiles()
        .into_iter()
        .map(|name| match profiles::load_profile(&name) {
            Ok(profile) => json!({
                "name": name,
                "description": profile.get("description").and_then(Value::as_str).unwrap_or(""),
            }),
            Err(exc) => json!({ "name": name, "error": exc.to_string() }),
        })
        .collect();
    if as_json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("(no profiles found)");
        return Ok(());
    }
    for row in &rows {
        let name = row["name"].as_str().unwrap_or_default();
        if let Some(error) = row["error"].as_str() {
            println!("{name:<24} (load error: {error})");
            continue;
        }
        let description = row["description"].as_str().unwrap_or_default();
        let first_sentence: String = description
            .split('.')
            .next()
            .unwrap_or("")
            .to_string();
        println!("{name:<24} {first_sentence}");
    }
    Ok(())
}
