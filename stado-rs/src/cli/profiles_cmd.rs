//! `stado profiles [NAME]` — port of the `profiles` command in
//! `stado/cli.py`: list visible profiles with their one-line descriptions,
//! or dump one profile's JSON.

use serde_json::Value;

use crate::profiles;

use super::CmdError;

pub fn run(name: Option<&str>) -> Result<(), CmdError> {
    if let Some(name) = name {
        let profile = profiles::load_profile(name).map_err(|exc| match exc {
            profiles::ProfileError::NotFound(_) | profiles::ProfileError::Invalid(_) => {
                CmdError::click(exc.to_string())
            }
            other => CmdError::from(other),
        })?;
        println!("{}", serde_json::to_string_pretty(&Value::Object(profile))?);
        return Ok(());
    }
    let names = profiles::list_profiles();
    if names.is_empty() {
        println!("(no profiles found)");
        return Ok(());
    }
    for name in names {
        match profiles::load_profile(&name) {
            Ok(profile) => {
                let description = profile
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let first_sentence = description.split('.').next().unwrap_or("");
                let first_sentence: String = first_sentence.chars().take(90).collect();
                println!("{name:<24} {first_sentence}");
            }
            Err(exc) => println!("{name:<24} (load error: {exc})"),
        }
    }
    Ok(())
}
