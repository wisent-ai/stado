//! `config set` and `config unset`: the two verbs that change one dotted key
//! of the configuration file. Both walk the document by key segment, validate
//! the result before anything is written, and land the new document with the
//! same atomic rename.

use serde_json::{Map, Value};

use crate::config_file;

use crate::cli::CmdError;

/// `config set KEY VALUE`: change one dotted key in the config file.
///
/// Enabling an alert channel, pointing a URL at a live listener, or naming a
/// destination used to mean hand-editing the deployment's JSON. The document
/// is validated before it is written and the write is atomic, so a rejected
/// value leaves the running configuration exactly as it was.
pub(super) fn set(key: &str, raw: &str) -> Result<(), CmdError> {
    let path = config_file::config_path()
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists; run: stado config init"))?;
    let original = std::fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&original)?;
    if !document.is_object() {
        return Err(CmdError::click("config file must contain a JSON object"));
    }
    // A bare word is what an operator types for a string value; anything that
    // parses as JSON keeps its type, so lists and booleans need no quoting
    // dance.
    let parsed: Value = serde_json::from_str(raw).unwrap_or_else(|_| Value::from(raw));

    let mut cursor = &mut document;
    let segments: Vec<&str> = key.split('.').collect();
    let (last, parents) = segments
        .split_last()
        .ok_or_else(|| CmdError::click("config set needs a non-empty key"))?;
    for segment in parents {
        let object = cursor
            .as_object_mut()
            .ok_or_else(|| CmdError::click(format!("{key}: {segment} is not an object")))?;
        cursor = object
            .entry((*segment).to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let object = cursor
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("{key}: parent is not an object")))?;
    let previous = object.insert((*last).to_string(), parsed.clone());

    let problems = config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "{key} rejected, config unchanged: {}",
            problems.join("; ")
        )));
    }

    let body = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let temporary = std::path::PathBuf::from(format!("{}.setting", path.display()));
    std::fs::write(&temporary, body)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    println!(
        "{key}: {} -> {} ({})",
        previous.unwrap_or(Value::Null),
        parsed,
        path.display()
    );
    Ok(())
}

/// `config unset KEY`: remove one dotted key from the config file.
///
/// [`set`] can only add a key or change one, so a declaration that outlived
/// its reader could not be retired through this product at all. That is how
/// `storage.stado.ca_file` survived in this deployment: unreachable behind a
/// loopback `storage.stado.url`, reported by `registry doctor` every run, and
/// removable only by hand-editing the document `set` exists to stop people
/// hand-editing.
///
/// Same validation and the same atomic write as [`set`]: a removal that would
/// leave the document invalid changes nothing. An absent key is reported and
/// succeeds, so a converge pass can run this twice.
pub(super) fn unset(key: &str) -> Result<(), CmdError> {
    let path = config_file::config_path()
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .ok_or_else(|| CmdError::click("no config file exists; run: stado config init"))?;
    let original = std::fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&original)?;
    if !document.is_object() {
        return Err(CmdError::click("config file must contain a JSON object"));
    }

    let segments: Vec<&str> = key.split('.').collect();
    let (last, parents) = segments
        .split_last()
        .ok_or_else(|| CmdError::click("config unset needs a non-empty key"))?;
    let mut cursor = &mut document;
    for segment in parents {
        let object = cursor
            .as_object_mut()
            .ok_or_else(|| CmdError::click(format!("{key}: {segment} is not an object")))?;
        match object.get_mut(*segment) {
            Some(next) => cursor = next,
            // Nothing to remove, and no parent to invent: saying so is the
            // whole answer.
            None => {
                println!("{key}: not present ({})", path.display());
                return Ok(());
            }
        }
    }
    let object = cursor
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("{key}: parent is not an object")))?;
    let Some(previous) = object.remove(*last) else {
        println!("{key}: not present ({})", path.display());
        return Ok(());
    };

    let problems = config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "{key} is required, config unchanged: {}",
            problems.join("; ")
        )));
    }

    let body = format!("{}\n", serde_json::to_string_pretty(&document)?);
    let temporary = std::path::PathBuf::from(format!("{}.unsetting", path.display()));
    std::fs::write(&temporary, body)?;
    if let Ok(metadata) = std::fs::metadata(&path) {
        std::fs::set_permissions(&temporary, metadata.permissions())?;
    }
    std::fs::rename(&temporary, &path)?;
    println!("{key}: {previous} removed ({})", path.display());
    Ok(())
}
