//! `config validate`: the structural check over the loaded document, and the
//! exit status the check answers with.

use serde_json::Value;

use crate::config_file;

use crate::cli::CmdError;

/// `config validate`: structural check; problems print as `ERROR ...`
/// lines and exit 1 (Python `raise SystemExit(1)`).
pub(in crate::cli::config_cmd) fn validate() -> Result<(), CmdError> {
    let data = config_file::load_config_file().map_err(|exc| CmdError::click(exc.to_string()))?;
    let problems = config_file::validate(&Value::Object(data.clone()));
    if !problems.is_empty() {
        for problem in problems {
            println!("ERROR {problem}");
        }
        return Err(CmdError::silent(1));
    }
    let where_ = config_file::config_path().map_err(|exc| CmdError::click(exc.to_string()))?;
    match where_ {
        Some(path) => println!("config ok ({})", path.display()),
        None => println!("config ok (defaults; no config file)"),
    }
    Ok(())
}
