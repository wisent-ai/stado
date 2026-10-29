//! `config validate`: the structural check over the loaded document, and the
//! exit status the check answers with.

use serde_json::Value;

use crate::cli::resolver::HostResolver;
use crate::config_file;

use crate::cli::CmdError;

/// `config validate`: structural check, then the declarations this host's
/// running roles need from it; problems print as `ERROR ...` lines and exit
/// 1 (Python `raise SystemExit(1)`). An incoming Stado runs this before it
/// replaces the installed one, so a declaration a new release requires
/// refuses the install instead of stopping the host's resolver. A resolver
/// state file whose publisher no longer runs is no resolver: it is said in a
/// `NOTE ...` line, which the install relays, and asks nothing of the
/// configuration.
pub(in crate::cli::config_cmd) fn validate() -> Result<(), CmdError> {
    let data = config_file::load_config_file().map_err(CmdError::from)?;
    let mut problems = config_file::validate(&Value::Object(data.clone()));
    match crate::cli::resolver::host_resolver() {
        HostResolver::Running => {
            problems.extend(crate::deploy::host_access::native::long_lived_refusal());
        }
        HostResolver::Absent => {}
        HostResolver::Exited(said) => println!(
            "NOTE {said}; the long-lived SSH session a resolver holds is not asked of this \
             configuration"
        ),
    }
    if !problems.is_empty() {
        for problem in problems {
            println!("ERROR {problem}");
        }
        return Err(CmdError::silent(1));
    }
    let where_ = config_file::config_path().map_err(CmdError::from)?;
    match where_ {
        Some(path) => println!("config ok ({})", path.display()),
        None => println!("config ok (defaults; no config file)"),
    }
    Ok(())
}
