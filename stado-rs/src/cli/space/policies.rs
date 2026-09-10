//! `stado space policies [TARGET]`: the memory policies this fleet declares,
//! and, for one host, which of them fit it and which one it is carrying.
//!
//! It answers the question an operator actually has in front of a machine
//! that ran out of memory: not "what are the fields of a memory policy" but
//! "is this host managed, by which declaration, and is that declaration one
//! we reviewed". A host whose registry carries a hand-typed policy reads as
//! `unreviewed` here rather than as managed, because a document nobody can
//! find in git is a document nobody maintains.

use serde_json::{json, Value};

use super::{print_json, CmdError};
use crate::providers::local::host_memory::declaration::policies::{self, DeclaredPolicy};

/// What one declared policy looks like as a row.
fn policy_json(policy: &DeclaredPolicy) -> Value {
    json!({
        "name": policy.name,
        "summary": policy.summary,
        "platforms": policy.platforms,
        "roles": policy.roles,
        "mode": policy.policy.mode,
        "repairs": policy.policy.repairs.keys().collect::<Vec<&String>>(),
        "refuse_placement": policy.policy.refuse_placement,
        "ends_graphical_session": policy.ends_graphical_session(),
        "session_processes": policy.session_processes(),
        "policy": policy.policy,
    })
}

/// Which declared policy this document IS, when it is one of them.
pub fn matching_name(declared: &Value) -> Option<&'static str> {
    let policies = policies::all().ok()?;
    policies
        .iter()
        .find(|candidate| {
            serde_json::to_value(&candidate.policy).is_ok_and(|document| &document == declared)
        })
        .map(|candidate| candidate.name.as_str())
}

/// Whether a host's declaration repairs anything, and the sentence saying why.
pub fn automatic_verdict(declared: Option<&Value>) -> Value {
    let Some(document) = declared else {
        return json!({
            "armed": false,
            "reviewed_policy": null,
            "detail": "declares no memory_reclaim policy, so it is measured against the \
                       reporting default, which reports and repairs nothing",
        });
    };
    let mode = document.get("mode").and_then(Value::as_str).unwrap_or("");
    let repairs: Vec<&String> = document
        .get("repairs")
        .and_then(Value::as_object)
        .map(|repairs| repairs.keys().collect())
        .unwrap_or_default();
    let reviewed = matching_name(document);
    let armed = mode == "enforce" && !repairs.is_empty();
    let detail = if armed {
        format!(
            "enforces its watermarks and may perform {}",
            repairs
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<&str>>()
                .join(", ")
        )
    } else if mode == "enforce" {
        "enforces its watermarks and names no repair, so it reports pressure it cannot act on"
            .to_string()
    } else {
        format!("declares mode {mode:?}, so no repair is ever performed on this host")
    };
    json!({
        "armed": armed,
        "reviewed_policy": reviewed,
        "mode": mode,
        "repairs": repairs,
        "detail": detail,
    })
}

/// `space policies` body.
pub async fn dispatch(target: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let declared = policies::all().map_err(CmdError::click)?;
    let rows: Vec<Value> = declared.iter().map(policy_json).collect();

    let Some(name) = target else {
        if json_output {
            return print_json(&json!({
                "declaration": policies::DECLARATION_PATH,
                "policies": rows,
            }));
        }
        println!("{} declares:", policies::DECLARATION_PATH);
        for policy in declared {
            print_policy(policy);
        }
        return Ok(());
    };

    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let entry = registry
        .lookup(name)
        .ok_or_else(|| CmdError::click(format!("target not in registry: {name}")))?;
    let role = entry.role.as_deref();
    let fitting = policies::fitting(&entry.release_platform, role);
    let carried = entry
        .memory_reclaim
        .as_ref()
        .map(serde_json::to_value)
        .transpose()?;
    let verdict = automatic_verdict(carried.as_ref());

    if json_output {
        return print_json(&json!({
            "declaration": policies::DECLARATION_PATH,
            "target": name,
            "release_platform": entry.release_platform,
            "role": role,
            "automatic": verdict,
            "fitting": fitting.iter().map(|policy| policy_json(policy)).collect::<Vec<Value>>(),
            "policies": rows,
        }));
    }

    println!(
        "{name}: {} {} — {}",
        entry.release_platform,
        role.unwrap_or("no declared role"),
        verdict
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or_default()
    );
    match verdict.get("reviewed_policy").and_then(Value::as_str) {
        Some(reviewed) => println!("  carries the declared policy {reviewed}"),
        None if carried.is_some() => println!(
            "  carries a policy that is not one of {}; it was written by hand and nothing \
             reviews it",
            policies::DECLARATION_PATH
        ),
        None => {}
    }
    if fitting.is_empty() {
        println!(
            "  no declared policy is written for this platform and role; {} declares {}",
            policies::DECLARATION_PATH,
            policies::declared_names().join(", ")
        );
        return Ok(());
    }
    println!("  policies written for this host:");
    for policy in fitting {
        print_policy(policy);
    }
    Ok(())
}

fn print_policy(policy: &DeclaredPolicy) {
    let repairs: Vec<&str> = policy
        .policy
        .repairs
        .keys()
        .map(String::as_str)
        .collect::<Vec<&str>>();
    println!(
        "  {} [{}] {} on {} — {}",
        policy.name,
        policy.policy.mode,
        if repairs.is_empty() {
            "no repair".to_string()
        } else {
            repairs.join(", ")
        },
        policy.platforms.join(", "),
        policy.summary
    );
    if policy.ends_graphical_session() {
        println!(
            "    ends session processes {}; applying it needs --authorize-graphical-session",
            policy.session_processes().join(", ")
        );
    }
}
