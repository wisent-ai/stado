//! `stado waits`: the waits open on this machine, read from the register
//! every Stado wait keeps. A wait whose process no longer runs is reported
//! ended, with the line it held when its process died, and that reading
//! removes it from the register.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::wait::open::{self, Open, RegisterError};

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
