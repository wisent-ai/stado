//! A newer installation superseding an older one of the same surface: the
//! older holder past placement, and the older installation still running its
//! after-install steps without the lock. Each is told to stop with every
//! process under it, each said on stderr, and the receipt records what was
//! superseded, so the older one's steps neither finish after the newer
//! installation's own nor stand in the receipt as still running.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use crate::common::{checked, now, Runtime};
use crate::state::ProductState;

/// The receipt field an after-install run superseded by a newer installation
/// is recorded under, in place of `after_install_running`.
const SUPERSEDED: &str = "after_install_superseded";
/// The receipt field an after-install run that ended without recording its
/// outcome is recorded under, in place of `after_install_running`.
const INTERRUPTED: &str = "after_install_interrupted";

/// An installation that placed its files lets go of the lock while its
/// after-install steps run, so a newer installation of the same surface takes
/// the lock at once; that is where it supersedes the older one too. Once this
/// process holds the lock, an older installation the receipt records as
/// running its after-install steps, whose PID the process table still names
/// with the recorded birth time, is told to stop with every process under it,
/// each said on stderr, so its steps (a converge of the readers of the binary
/// it placed) do not finish after this installation's own. A recorded PID
/// with another birth time is another process, and nothing is signalled. In
/// both cases the receipt's `after_install_running` is replaced by what
/// happened to it: superseded by this installation, with every process told,
/// or ended without recording its outcome.
pub(in crate::install) fn stop_after_install(
    runtime: &Runtime,
    id: &str,
    surface: &str,
) -> Result<()> {
    let Some(operation) = super::post_placement(runtime, id, surface)? else {
        return Ok(());
    };
    let pid = i32::try_from(
        operation["pid"]
            .as_i64()
            .context("after-install operation names no pid")?,
    )?;
    let (field, record) = if operation["running"] == true {
        eprintln!(
            "superseding the installation of {id}/{surface} by pid {pid}: it is running its after-install steps {}",
            operation["steps"]
        );
        let told: Vec<Value> = descendants(pid)?
            .into_iter()
            .map(|(child, _)| child)
            .chain([pid])
            .map(stop)
            .collect();
        for each in &told {
            eprintln!("  {each}");
        }
        let by = i32::try_from(std::process::id())?;
        (SUPERSEDED, json!({"superseded_by": by, "told": told}))
    } else {
        (
            INTERRUPTED,
            json!({"observed": "the recorded pid no longer runs with its recorded birth time, and its steps recorded no outcome"}),
        )
    };
    let mut receipt = ProductState::load(runtime, id, surface)?
        .with_context(|| format!("{id}/{surface} recorded an after-install run but no receipt"))?;
    let Some(Value::Object(mut marker)) = receipt.extra.remove("after_install_running") else {
        anyhow::bail!("{id}/{surface}'s after_install_running is not a record");
    };
    let Value::Object(added) = record else {
        unreachable!("the record is built as an object above")
    };
    marker.extend(added);
    marker.insert("at".to_owned(), json!(now()));
    receipt
        .extra
        .insert(field.to_owned(), Value::Object(marker));
    receipt.save(runtime)
}

/// Every process under `pid`, its children's children included, with their
/// command lines, as the process table lists them now, deepest first: the
/// step an installation runs (a build, a dependency's installation, an
/// after-install reconciler) and whatever that step started.
fn descendants(pid: i32) -> Result<Vec<(i32, String)>> {
    let listed = checked(Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid=,command="]))
        .context("the process table could not be read to find the operation's steps")?;
    let mut under: BTreeMap<i32, Vec<(i32, String)>> = BTreeMap::new();
    for line in String::from_utf8_lossy(&listed.stdout).lines() {
        let mut words = line.split_whitespace();
        let (Some(child), Some(parent)) = (words.next(), words.next()) else {
            continue;
        };
        let child: i32 = child
            .parse()
            .with_context(|| format!("ps printed a pid that is no pid: {line}"))?;
        let parent: i32 = parent
            .parse()
            .with_context(|| format!("ps printed a parent that is no pid: {line}"))?;
        under
            .entry(parent)
            .or_default()
            .push((child, words.collect::<Vec<_>>().join(" ")));
    }
    let mut found = Vec::new();
    let mut frontier = vec![pid];
    while let Some(parent) = frontier.pop() {
        for (child, command) in under.remove(&parent).into_iter().flatten() {
            frontier.push(child);
            found.push((child, command));
        }
    }
    found.reverse();
    Ok(found)
}

/// Whether `command` runs the after-install step `step` (the recipe's argv):
/// its program is the step's program, and every literal word of the step
/// (not a placeholder, not an option, whose value may be one) follows in
/// order. A source installation leaves out an option whose value is a
/// release placeholder, so options are not required.
fn runs_step(command: &str, step: &Value) -> bool {
    let Some(words) = step.as_array() else {
        return false;
    };
    let mut ran = command.split_whitespace();
    let Some(program) = ran.next() else {
        return false;
    };
    let wanted: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
    let Some((first, rest)) = wanted.split_first() else {
        return false;
    };
    if Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(first)
    {
        return false;
    }
    let mut ran = ran.peekable();
    rest.iter()
        .filter(|word| !word.contains('{') && !word.starts_with("--"))
        .all(|word| {
            while ran.peek().is_some_and(|seen| seen != word) {
                ran.next();
            }
            ran.next().is_some()
        })
}

/// A holder from a Stado that records no end of placement keeps `placing`
/// while it runs its recipe's after-install steps, which do not change the
/// surface, so a newer installation waited on it for as long as such a step
/// stood (a converge reading another host). When the holder of `lock` says
/// `placing` and a process under it runs one of `recipe`'s `after_install`
/// steps, it is past placement: it and every process under it are told to
/// stop, as a holder still preparing is, and the newer installation takes the
/// lock once they let go. Each process told is said on stderr.
pub(in crate::install) fn stop_past_placement(lock: &Path, recipe: &Value) -> Result<()> {
    let Some(steps) = recipe["after_install"].as_array() else {
        return Ok(());
    };
    let Some(held) = super::holder(lock)? else {
        return Ok(());
    };
    if held["phase"].as_str() != Some(crate::common::PHASE_PLACING) || held["running"] != true {
        return Ok(());
    }
    let pid = i32::try_from(held["pid"].as_u64().context("the holder names no pid")?)?;
    let running = descendants(pid)?;
    let Some((_, step)) = running
        .iter()
        .find(|(_, command)| steps.iter().any(|step| runs_step(command, step)))
    else {
        return Ok(());
    };
    eprintln!(
        "superseding the installation {}: it is past placement, running its after-install step `{step}`",
        super::clause(lock)
    );
    for told in running
        .iter()
        .map(|(child, _)| *child)
        .chain([pid])
        .map(stop)
    {
        eprintln!("  {told}");
    }
    Ok(())
}

/// Tell `pid` to stop; the answer names the error when it could not be told.
fn stop(pid: i32) -> Value {
    // SAFETY: a signal to the holder the lock record names, or to a process
    // the process table lists under it right now.
    let refused = unsafe { libc::kill(pid, libc::SIGTERM) }.is_negative();
    json!({"pid": pid, "told_to_stop": !refused,
        "error": refused.then(|| std::io::Error::last_os_error().to_string())})
}
