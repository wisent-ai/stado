//! Whether an action may run now, asked of Oko's one admission decision
//! (`oko admit`). Stado keeps no copy of any ruling: a refused action is
//! refused with the ruling Oko quotes, and admitted once a later ruling of
//! the operator lifts it.
//!
//! An admitted operation is one operation with everything it runs: the
//! quality gates of a build, the real journeys those gates start and the
//! dependency installs it makes. Oko's answer is kept in this process's
//! environment under [`inherited`]'s name for the action, so each process it
//! starts inherits it and is admitted for that action without asking again —
//! a gate of an admitted build does not hang on which Oko the machine has
//! installed, and an operation is not admitted twice by two answers.
//!
//! Oko answers with its verdict as JSON (`--json`): `allowed` and the
//! `reason` quoting what decided it. Only a verdict that says not allowed is
//! the operator's rulings refusing; an `oko` that gives no verdict at all (it
//! cannot be started, or it is an Oko without the `admit` verb) is said as
//! that, with its own words, since a ruling that was never read decided
//! nothing. A machine with no Oko installed at any of its declared paths has
//! no register of rulings at all: Stado runs without Oko like every product
//! runs without the others, so the action is admitted, saying that no Oko is
//! installed and so no ruling governs it.

use serde_json::Value;
use tokio::process::Command;

use crate::cli::CmdError;

use crate::deploy::host_exec::{installed_program, OKO_CLI, TAMA_CLI};

mod asked;
pub use asked::refusal_as;
use asked::scope;

/// The action class a ruling constrains that compiling a product is.
pub const BUILD: &str = "build";
/// The action class a ruling constrains that publishing a release is.
pub const RELEASE: &str = "release";
/// The action class a ruling constrains that installing a product is.
pub const INSTALL: &str = "install";
/// The action class a ruling constrains that a static check compiling or
/// analysing the source without building it is (a `lint` quality gate).
pub const COMPILE: &str = "compile";
/// The action class a ruling constrains that running a product's tests is
/// (a `test` quality gate).
pub const TEST: &str = "test";

/// The environment variable that carries an admitted operation's admission
/// for `action` to the processes it runs: the admitted process's pid, a
/// space, and Oko's answer.
pub fn inherited(action: &str) -> String {
    format!("STADO_ADMITTED_{}", action.to_uppercase())
}

/// The parent and program of process `pid` as `ps` reads them; none when no
/// such process runs.
async fn process_row(pid: &str) -> Result<Option<(String, String)>, String> {
    let shown = format!("ps -o ppid= -o comm= -p {pid}");
    let mut command = Command::new("ps");
    command
        .args(["-o", "ppid=", "-o", "comm=", "-p", pid])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = crate::wait::until(
        crate::wait::Kind::Process,
        &shown,
        "the process table",
        command.output(),
    )
    .await
    .map_err(|error| format!("{shown} did not run: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let row = text.trim();
    match (output.status.success(), row.split_once(char::is_whitespace)) {
        (true, Some((parent, program))) => {
            Ok(Some((parent.trim().to_owned(), program.trim().to_owned())))
        }
        (false, _) if row.is_empty() => Ok(None),
        _ => Err(format!(
            "{shown} exited {} with {row:?}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// Whether `pid` is a running ancestor of this process that runs Stado:
/// the admission a value names is honoured only from the Stado operation
/// that was admitted and started this one, never from a value typed by hand
/// that names any other process.
async fn stado_ancestor(pid: &str) -> Result<bool, String> {
    let own = std::env::current_exe()
        .map_err(|error| format!("this Stado's own executable cannot be read: {error}"))?;
    let mut current = std::os::unix::process::parent_id().to_string();
    loop {
        let Some((parent, program)) = process_row(&current).await? else {
            return Ok(false);
        };
        if current == pid {
            return Ok(std::path::Path::new(&program).file_name() == own.file_name());
        }
        if parent == current {
            return Ok(false);
        }
        current = parent;
    }
}

/// Whether a process that started this one was admitted for the action
/// `variable` carries. This process's own earlier admission is not one: each
/// of its operations (a delivery plan's every step) is asked. A value Stado
/// did not write (not Unicode, no pid), or one naming a process that is not
/// a running Stado among this process's ancestors, admits nothing, so Oko is
/// asked; a process table that cannot be read is said, and Oko is asked.
async fn admitted_above(variable: &str) -> bool {
    let value = match std::env::var(variable) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent | std::env::VarError::NotUnicode(_)) => return false,
    };
    let Some((pid, _answer)) = value.split_once(' ') else {
        return false;
    };
    if pid == std::process::id().to_string() {
        return false;
    }
    match stado_ancestor(pid).await {
        Ok(admitted) => admitted,
        Err(cause) => {
            eprintln!("stado: the admission {variable} names process {pid}, whose ancestry cannot be read ({cause}); Oko is asked");
            false
        }
    }
}

/// Ok when `action` may run now for `product` (every product when none is
/// named) as work serving the open defects `purposes` (`oko admit …
/// --purpose <defect>`: a consent the operator limited to a purpose covers
/// only work that names it); otherwise the refusal Oko gives, which names
/// the ruling, its words and what it waits on, prefixed with what was asked.
/// A process an admitted operation started is admitted by that operation for
/// the same action (module doc).
pub async fn refusal(action: &str, product: Option<&str>, purposes: &[&str]) -> Result<(), String> {
    let variable = inherited(action);
    if admitted_above(&variable).await {
        return Ok(());
    }
    // A consent the operator gave one session covers that session's work
    // only, so Oko is told which session runs this one.
    let session = calling_session().await.map_err(|cause| {
        format!(
            "{} is not run: the agent session it runs under could not be read ({cause})",
            scope(action, product)
        )
    })?;
    let answer = refusal_as(action, product, purposes, session.as_deref()).await?;
    // Set before the operation starts anything, so every process it runs
    // inherits it (module doc).
    std::env::set_var(variable, format!("{} {}", std::process::id(), answer));
    Ok(())
}

/// What `oko admit … --json` answered.
enum Decision {
    /// Allowed, with the reason Oko gives.
    Allowed(String),
    /// Not allowed, with the reason quoting the ruling.
    Refused(String),
    /// No verdict, and why.
    Unanswered(Unanswered),
}

/// Why an installed program (Oko, Tama) gave no JSON answer.
pub(crate) enum Unanswered {
    /// The program is installed at none of its declared paths, with where
    /// it was looked for.
    Absent(String),
    /// The program could not be run, or it answered something that is not
    /// JSON, with what it said.
    Unread(String),
}

/// The agent session this process runs under, as Tama's register of agent
/// sessions answers it (`tama sessions self --json`): none when no recorded
/// session holds this process or any of its ancestors (a command the operator
/// runs himself), and none on a machine with no Tama installed, which records
/// no session at all.
pub(crate) async fn calling_session() -> Result<Option<String>, String> {
    let words = ["sessions", "self", "--json"];
    match installed_json(TAMA_CLI, &words).await {
        Ok(answer) => match &answer["session"] {
            Value::String(session) => Ok(Some(session.clone())),
            Value::Null => Ok(None),
            other => Err(format!(
                "tama {} answered session {other}, neither a session id nor null",
                words.join(" ")
            )),
        },
        Err(Unanswered::Absent(_)) => Ok(None),
        Err(Unanswered::Unread(cause)) => Err(cause),
    }
}

/// What `oko <words>` printed as JSON, whatever it exited with (Oko prints a
/// refused verdict and exits non-zero), or why it printed none.
pub(crate) async fn oko_json(words: &[&str]) -> Result<Value, Unanswered> {
    installed_json(OKO_CLI, words).await
}

/// What the installed `program` printed as JSON for `words`, whatever it
/// exited with, or why it printed none.
async fn installed_json(program: &str, words: &[&str]) -> Result<Value, Unanswered> {
    // A home that is not known is not a machine without the program: where
    // it would be installed was never looked at.
    let installed = match (installed_program(program), crate::config_file::home()) {
        (Ok(installed), _) => installed,
        (Err(missing), Ok(_)) => return Err(Unanswered::Absent(missing)),
        (Err(missing), Err(cause)) => {
            return Err(Unanswered::Unread(format!(
                "{program} {} did not run: {missing}; {cause}",
                words.join(" ")
            )))
        }
    };
    let shown = format!("{} {}", installed.display(), words.join(" "));
    let mut command = Command::new(&installed);
    command
        .args(words)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = crate::wait::until(
        crate::wait::Kind::Process,
        &shown,
        "the caller's directory",
        command.output(),
    )
    .await
    .map_err(|error| Unanswered::Unread(format!("{shown} did not run: {error}")))?;
    serde_json::from_slice(&output.stdout).map_err(|error| {
        Unanswered::Unread(format!(
            "{shown} exited {} without an answer (its output is not JSON: {error}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    })
}

async fn decision(words: &[&str]) -> Decision {
    let verdict = match oko_json(words).await {
        Ok(verdict) => verdict,
        Err(unanswered) => return Decision::Unanswered(unanswered),
    };
    match (verdict["allowed"].as_bool(), verdict["reason"].as_str()) {
        (Some(true), Some(reason)) => Decision::Allowed(reason.to_owned()),
        (Some(false), Some(reason)) => Decision::Refused(reason.to_owned()),
        _ => Decision::Unanswered(Unanswered::Unread(format!(
            "oko {} answered no verdict: its JSON states no boolean allowed and string reason: {verdict}",
            words.join(" ")
        ))),
    }
}

/// [`refusal`] for a product operation that compiles or publishes, serving
/// the open defects `purposes` the caller named (none when it named none):
/// refused, nothing was exported or built.
pub async fn require(
    action: &str,
    product: Option<&str>,
    purposes: &[&str],
) -> Result<(), CmdError> {
    refusal(action, product, purposes)
        .await
        .map_err(|refused| CmdError::refused(format!("{refused}; nothing was exported or built")))
}
