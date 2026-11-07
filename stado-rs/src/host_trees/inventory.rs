//! `stado space report --unregistered [TARGET] --revision REV [--json]`: on
//! every host of the registry (or the one named), each tree at the top of the
//! home of a host the fleet owns, under `~/.stado`, under `~/.stado/work`,
//! under the declared work root and under every runner root that no register
//! entry holds. A tree any fleet writer made, by whatever spelling, and no
//! entry declares is one line here on the next run.
//!
//! The answer is about the register this binary carries, so it is refused
//! unless the binary was built from the revision the caller names.

use serde_json::{json, Value};

use super::{holder, runner_roots, Base};
use crate::binary::build_identity::{SOURCE_REVISION, UNKNOWN_REVISION};
use crate::cli::CmdError;
use crate::deploy::{host_channel, shlex_quote};
use crate::primitives::failure::FailureCode;
use crate::targets::{ComputeTarget, HostOwnership};

/// The listing program: one line per directory directly below each listed
/// directory, symbolic links left out, and one line for a listed directory
/// that is not there.
const LIST_FUNCTION: &str = r#"set -u
list() {
  [ -d "$2" ] || { printf 'STADO_TREE_ABSENT\t%s\n' "$1"; return 0; }
  for entry in "$2"/* "$2"/.[!.]* "$2"/..?*; do
    [ -d "$entry" ] && [ ! -L "$entry" ] || continue
    printf 'STADO_TREE\t%s\t%s\n' "$1" "${entry##*/}"
  done
}
"#;

/// One directory listed on a host: the base it is in the register, its
/// path below that base, the shell word naming it there, and how a report
/// names it.
struct Listing {
    base: Base,
    listed: String,
    word: String,
    shown: String,
}

/// Every directory listed on `target`.
fn listings(target: &ComputeTarget) -> Result<Vec<Listing>, String> {
    let home = |listed: &str| Listing {
        base: Base::Home,
        listed: listed.to_string(),
        word: if listed.is_empty() {
            "\"$HOME\"".to_string()
        } else {
            format!("\"$HOME\"/{}", shlex_quote(listed))
        },
        shown: if listed.is_empty() {
            "~".to_string()
        } else {
            format!("~/{listed}")
        },
    };
    let mut listings = Vec::new();
    // The top of the home is the fleet's only on a host it owns; on a
    // person's computer it is that person's.
    if target.ownership()? == HostOwnership::Dedicated {
        listings.push(home(""));
    }
    let work = crate::providers::local::work_base::HOME_WORK_COMPONENTS;
    let stado = work
        .split_last()
        .map(|(_, parent)| parent.join("/"))
        .unwrap_or_default();
    listings.push(home(stado.as_str()));
    listings.push(home(work.join("/").as_str()));
    if let Some(root) = target.work_root.as_deref() {
        listings.push(Listing {
            base: Base::WorkRoot,
            listed: String::new(),
            word: shlex_quote(root),
            shown: root.to_string(),
        });
    }
    for root in runner_roots() {
        listings.push(Listing {
            base: Base::Runner,
            listed: String::new(),
            word: shlex_quote(&root),
            shown: root,
        });
    }
    Ok(listings)
}

/// The unregistered trees of one host, or why they could not be read.
async fn host_instances(target: &ComputeTarget) -> Result<Vec<Value>, String> {
    let listings = listings(target)
        .map_err(|error| format!("its registry entry cannot be read: {error}"))?;
    let mut script = String::from(LIST_FUNCTION);
    for (index, listing) in listings.iter().enumerate() {
        script.push_str(&format!("list {index} {}\n", listing.word));
    }
    let runner = crate::deploy::production_runner();
    let output = host_channel::run_script(target, &script, &runner)
        .await
        .map_err(|error| format!("the listing could not run there: {error}"))?;
    if !output.succeeded() {
        return Err(format!("the listing failed there: {}", output.detail().trim()));
    }
    let declared = target.work_root.is_some();
    let mut instances = Vec::new();
    for line in output.stdout.lines() {
        let fields = host_channel::marker_fields(line);
        let ["STADO_TREE", index, child] = fields.as_slice() else {
            continue;
        };
        let Some(listing) = index.parse::<usize>().ok().and_then(|at| listings.get(at)) else {
            return Err(format!("the listing answered a line this command did not ask for: {line}"));
        };
        if holder(listing.base, &listing.listed, child, declared).is_none() {
            instances.push(json!({
                "host": target.name,
                "path": format!("{}/{child}", listing.shown),
                "fix": "declare the tree in stado-rs/src/host_trees/entries.rs with its writer and its reclaim, or stop the writer that made it",
            }));
        }
    }
    Ok(instances)
}

/// Refuse a binary not built from `revision`: the register it carries is
/// the one the answer is about.
fn built_from(revision: &str) -> Result<(), String> {
    let named = !revision.is_empty() && revision.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !named {
        return Err(format!(
            "--revision {revision:?} is not a commit; name the commit the installed stado was built from"
        ));
    }
    if SOURCE_REVISION == UNKNOWN_REVISION
        || SOURCE_REVISION.ends_with("-dirty")
        || !SOURCE_REVISION.starts_with(&revision.to_ascii_lowercase())
    {
        return Err(format!(
            "this stado was built from {SOURCE_REVISION}, not {revision}, so its register is not \
             the one asked about; install a build of {revision} or name the revision this binary \
             carries (`stado --version`)"
        ));
    }
    Ok(())
}

/// The command body.
pub(crate) async fn unregistered(
    target: Option<&str>,
    revision: &str,
    json: bool,
) -> Result<(), CmdError> {
    built_from(revision).map_err(|refusal| {
        CmdError::refused(refusal)
            .stating(FailureCode::Refused)
            .machine_readable(json)
    })?;
    let (registry, notice) = crate::targets::fetch_registry_or_last_good()
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "the registry naming the hosts to inventory cannot be read: {error}"
            ))
            .stating(FailureCode::InfraDown)
            .machine_readable(json)
        })?;
    if let Some(notice) = notice {
        crate::targets::report_registry_notice(&notice);
    }
    let hosts: Vec<&ComputeTarget> = registry
        .targets
        .iter()
        .filter(|candidate| target.is_none_or(|name| candidate.name == name))
        .filter(|candidate| {
            candidate.is_provider(crate::capabilities::ProviderId::Local)
                && (host_channel::target_is_this_host(candidate)
                    || candidate.has_ssh_connection())
        })
        .collect();
    if let (Some(name), true) = (target, hosts.is_empty()) {
        return Err(CmdError::click(format!(
            "{name} is not a local host with a channel in the registry; `stado host list` names them"
        ))
        .stating(FailureCode::NotFound)
        .machine_readable(json));
    }
    let mut instances = Vec::new();
    let mut unread = Vec::new();
    for host in &hosts {
        match host_instances(host).await {
            Ok(found) => instances.extend(found),
            Err(error) => unread.push(json!({"host": host.name, "error": error})),
        }
    }
    let answer = json!({
        "revision": SOURCE_REVISION,
        "hosts": hosts.iter().map(|host| host.name.as_str()).collect::<Vec<_>>(),
        "instances": instances,
        "unread": unread,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&answer)?);
    } else {
        for instance in &instances {
            println!(
                "unregistered: {} {}",
                instance["host"].as_str().unwrap_or_default(),
                instance["path"].as_str().unwrap_or_default()
            );
        }
        for host in &unread {
            println!(
                "unread: {} {}",
                host["host"].as_str().unwrap_or_default(),
                host["error"].as_str().unwrap_or_default()
            );
        }
        println!(
            "{} unregistered tree(s) on {} host(s); {} host(s) unread",
            instances.len(),
            hosts.len(),
            unread.len()
        );
    }
    if unread.is_empty() {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "{} host(s) could not be inventoried, so the answer is incomplete: {}",
            unread.len(),
            unread
                .iter()
                .map(|host| format!("{}: {}", host["host"], host["error"]))
                .collect::<Vec<_>>()
                .join("; ")
        ))
        .stating(FailureCode::InfraDown)
        .machine_readable(json))
    }
}
