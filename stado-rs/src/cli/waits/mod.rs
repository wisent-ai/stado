//! `stado waits`: the waits open on this machine, read from the register
//! every Stado wait keeps. A wait whose process no longer runs is reported
//! ended, with the line it held when its process died, and that reading
//! removes it from the register. `stado waits open` registers a wait for
//! another process — one a program does not perform in Rust itself, such
//! as a grant dialog or a terminal read — and `stado waits close` removes
//! one.

use serde_json::{json, Value};

use crate::cli::entry::spec::root::installation::WaitsAction;
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::wait::open::{self, Open, RegisterError};
use crate::wait::{external, Kind};

fn refusal(error: RegisterError) -> CmdError {
    let code = match error.io {
        Some(kind) => crate::cli::entry::error::io_failure_code(kind),
        None => FailureCode::Config,
    };
    CmdError::click(format!(
        "the register of open waits cannot be read: {error}"
    ))
    .stating(code)
}

fn rows(waits: &[Open]) -> Value {
    Value::Array(
        waits
            .iter()
            .map(|wait| json!({"id": wait.id, "pid": wait.pid, "line": wait.line}))
            .collect(),
    )
}

/// `stado waits [open|close]`.
pub fn dispatch(action: Option<WaitsAction>, as_json: bool) -> Result<(), CmdError> {
    match action {
        None => run(as_json),
        Some(WaitsAction::Open {
            kind,
            what,
            place,
            pid,
            json,
        }) => register(&kind, &what, &place, pid, json || as_json),
        Some(WaitsAction::Close { id, json }) => close(&id, json || as_json),
    }
}

fn written(error: RegisterError) -> CmdError {
    let code = match error.io {
        Some(kind) => crate::cli::entry::error::io_failure_code(kind),
        None => FailureCode::Config,
    };
    CmdError::click(format!(
        "the register of open waits cannot be written: {error}"
    ))
    .stating(code)
}

fn register(kind: &str, what: &str, place: &str, pid: u32, as_json: bool) -> Result<(), CmdError> {
    let Some(kind) = Kind::from_word(kind) else {
        let words: Vec<&str> = Kind::ALL.iter().map(|kind| kind.word()).collect();
        return Err(CmdError::usage(format!(
            "stado waits open --kind takes one of the register's kinds, {}; got {kind}",
            words.join(", ")
        )));
    };
    for (flag, value) in [("--what", what), ("--where", place)] {
        if value.trim().is_empty() {
            return Err(CmdError::usage(format!(
                "stado waits open {flag} names what the wait is for and where; it was empty"
            )));
        }
    }
    let id = external::register_for(pid, kind, what, place).map_err(written)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "id": id, "pid": pid, "kind": kind.word(), "what": what, "where": place,
            }))?
        );
    } else {
        println!(
            "registered {id}: process {pid} waits on {} for {what} at {place}",
            kind.word()
        );
    }
    Ok(())
}

fn close(id: &str, as_json: bool) -> Result<(), CmdError> {
    let held = external::unregister(id).map_err(written)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "id": id, "removed": held }))?
        );
    } else if held {
        println!("closed {id}");
    } else {
        println!("{id} was no longer in the register: it had ended or its process was gone");
    }
    Ok(())
}

pub fn run(as_json: bool) -> Result<(), CmdError> {
    let directory =
        open::directory().map_err(|cause| refusal(RegisterError { cause, io: None }))?;
    let listing = open::listed().map_err(refusal)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "register": directory.display().to_string(),
                "open": rows(&listing.open),
                "ended": rows(&listing.ended),
            }))?
        );
        return Ok(());
    }
    println!(
        "register {}: {} open, {} ended",
        directory.display(),
        listing.open.len(),
        listing.ended.len()
    );
    for wait in &listing.open {
        println!("  open  {}", wait.line);
    }
    for wait in &listing.ended {
        println!(
            "  ended {} (process {} no longer runs; removed from the register)",
            wait.line, wait.pid
        );
    }
    Ok(())
}
