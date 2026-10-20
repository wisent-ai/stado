//! `stado waits hold | release | holds`: what a running process holds in the
//! wait-for graph, recorded by whoever knows it, so a wait on that resource
//! is a wait on that process.

use serde_json::json;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::wait::cycle::holds;
use crate::wait::Resource;

use super::written;

/// The resource `<kind>:<id>` names, or the usage refusal saying why not.
pub(super) fn resource(flag: &str, text: &str) -> Result<Resource, CmdError> {
    Resource::parse(text).map_err(|cause| CmdError::usage(format!("stado waits {flag}: {cause}")))
}

pub(super) fn hold(text: &str, pid: u32, as_json: bool) -> Result<(), CmdError> {
    let resource = resource("hold --resource", text)?;
    let id = holds::hold_for(pid, &resource).map_err(written)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "id": id, "pid": pid, "resource": resource.key(),
            }))?
        );
    } else {
        println!("recorded {id}: process {pid} holds {resource}");
    }
    Ok(())
}

pub(super) fn release(id: &str, as_json: bool) -> Result<(), CmdError> {
    let held = holds::release(id).map_err(written)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "id": id, "removed": held }))?
        );
    } else if held {
        println!("released {id}");
    } else {
        println!("{id} was no longer recorded: it had been released or its process was gone");
    }
    Ok(())
}

pub(super) fn list(as_json: bool) -> Result<(), CmdError> {
    let held = holds::held().map_err(|cause| {
        CmdError::click(format!(
            "the holdings of this machine's wait-for graph cannot be read: {cause}"
        ))
        .stating(FailureCode::Config)
    })?;
    if as_json {
        let rows: Vec<_> = held
            .iter()
            .map(|holding| {
                json!({"id": holding.id, "pid": holding.pid, "resource": holding.resource.key()})
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "holds": rows }))?
        );
        return Ok(());
    }
    println!("{} holding(s) of running processes", held.len());
    for holding in &held {
        println!(
            "  {}  process {} holds {}",
            holding.id, holding.pid, holding.resource
        );
    }
    Ok(())
}
