//! `config validate`: the structural check over the loaded document, and the
//! exit status the check answers with.

use serde_json::Value;

use crate::cli::resolver::HostResolver;
use crate::config_file;

use crate::cli::CmdError;

/// `config validate`: structural check, then the declarations this host's
/// running roles need from it; problems print as `ERROR ...` lines and exit
/// 1 (Python `raise SystemExit(1)`). An incoming Stado runs this with
/// `install` before it replaces the installed one, so a declaration a new
/// release requires refuses the install instead of stopping the host's
/// resolver. A resolver state file whose publisher no longer runs is no
/// resolver: it is said in a `NOTE ...` line, which the install relays, and
/// asks nothing of the configuration.
///
/// A declared public edge must sit on a host the registry holds, since every
/// hostname it terminates is delivered to that host's Stado. The registry is
/// read without its authority (the store on this disk, else this host's
/// last-known-good copy), so an install's validation never waits on a remote
/// read; a host that holds neither says so in a `NOTE` and is not refused.
/// An edge on a host the registry does not hold is an `ERROR` here, and with
/// `install` a `NOTE`: the incoming binary runs with that declaration as the
/// installed one does, and the commands that publish through the edge refuse
/// it by name, so refusing the install only held every later release, and
/// every repair it carries, behind a web publication setting.
pub(in crate::cli::config_cmd) async fn validate(install: bool) -> Result<(), CmdError> {
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
    if let Ok(edge) = crate::config::web_api_edge() {
        match crate::targets::registry_without_authority("checking the public edge's host").await {
            Ok((registry, _)) => {
                if matches!(registry.lookup_identity(edge.target()), Ok(None)) {
                    let refusal = crate::cli::web::unregistered_edge_refusal(edge.target());
                    if install {
                        println!("NOTE {refusal}; an install runs with this declaration, so it does not refuse one");
                    } else {
                        problems.push(refusal);
                    }
                }
            }
            Err(cause) => println!(
                "NOTE the public edge's host {} was not checked against the registry: {cause}",
                edge.target()
            ),
        }
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
