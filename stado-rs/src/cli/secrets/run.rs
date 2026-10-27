//! `stado credentials run --env NAME=ITEM#FIELD … -- PROGRAM [ARGS…]`: start
//! one program with credential fields in its environment and nowhere else.
//!
//! A workload reads its own credential and nobody else's, and a gateway
//! bearer reaches it from a launcher that owns it. This is that launcher as
//! a primitive: each `--env` names one variable and the field it takes, read
//! from the selected store or, with `--route`, under one consumer's own
//! grant. The values go into the started program's environment only: never
//! argv, a file or stdout. Stado then replaces itself with the program
//! (`execvp`), so the program's exit status, signals and terminal are its
//! own, and no Stado process outlives the read.

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;

use crate::cli::CmdError;

use super::store::items::{field_value, item_in_role, Store};

/// Where one variable's field is read from: an item named by its id, or the
/// one live item playing a role.
enum Holder {
    Item(String),
    Role(String),
}

/// One `--env NAME=ITEM#FIELD`.
struct Binding {
    variable: String,
    holder: Holder,
    field: String,
}

/// The form `--env` takes, said in every refusal of one.
const FORM: &str = "NAME=ITEM#FIELD or NAME=role:ROLE#FIELD";

/// Whether `name` is a variable name a shell exports: a letter or `_`, then
/// letters, digits and `_`.
fn exportable(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && characters.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// Read one `--env` value, refusing a malformed one by name: the variable
/// must be one a shell exports, and the holder and field must both be
/// present.
fn binding(spec: &str) -> Result<Binding, CmdError> {
    let refused = |why: &str| CmdError::usage(format!("--env {spec:?}: {why}; write it as {FORM}"));
    let (variable, source) = spec
        .split_once('=')
        .ok_or_else(|| refused("it names no variable"))?;
    if !exportable(variable) {
        return Err(refused("the variable is not a name a shell exports"));
    }
    let (holder, field) = source
        .rsplit_once('#')
        .ok_or_else(|| refused("it names no field"))?;
    if holder.is_empty() || field.is_empty() {
        return Err(refused("the item and the field must both be named"));
    }
    let holder = match holder.strip_prefix("role:") {
        Some(role) if !role.is_empty() => Holder::Role(role.to_string()),
        Some(_) => return Err(refused("the role is empty")),
        None => Holder::Item(holder.to_string()),
    };
    Ok(Binding {
        variable: variable.to_string(),
        holder,
        field: field.to_string(),
    })
}

/// Every binding read from `store`, each variable once. `delegated` says the
/// store is one consumer's grant, which cannot list the vault, so a role is
/// asked of Skarbiec as `role:<role>`; `reader` names who reads, in a
/// failed read's cause.
async fn values(
    store: &Store,
    delegated: bool,
    env: &[String],
    reader: &str,
) -> Result<BTreeMap<String, String>, CmdError> {
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for spec in env {
        let binding = binding(spec)?;
        if values.contains_key(&binding.variable) {
            return Err(CmdError::usage(format!(
                "--env names {} twice; each variable takes one field",
                binding.variable
            )));
        }
        let item = match binding.holder {
            Holder::Item(id) => id,
            Holder::Role(role) if delegated => format!("role:{role}"),
            Holder::Role(role) => item_in_role(store, &role).await?,
        };
        let value = field_value(store, &item, &binding.field)
            .await
            .map_err(|error| error.within(format!("{} for {reader}", binding.variable)))?;
        values.insert(binding.variable, value);
    }
    Ok(values)
}

/// The fields `env` names (`NAME=ITEM#FIELD`, as `--env` writes them), read
/// from the selected store, or with `route` under one consumer's own grant:
/// the read `credentials run` makes, for another launcher such as `sandbox
/// fleet exec --credential`.
pub(crate) async fn granted(
    env: &[String],
    route: Option<String>,
    consumer: Option<String>,
    grant_file: Option<String>,
) -> Result<BTreeMap<String, String>, CmdError> {
    let delegated = route.is_some();
    let store = super::commands::dispatch::delegated_or_selected(route, consumer, grant_file)?;
    values(&store, delegated, env, "the sandboxed command").await
}

/// Read every binding from `store`, then become `command` with them in its
/// environment. `delegated` says the store is one consumer's grant.
/// Returns only when the program could not be started.
pub(in crate::cli::secrets) async fn run(
    store: &Store,
    delegated: bool,
    env: &[String],
    command: &[String],
) -> Result<(), CmdError> {
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| CmdError::usage("credentials run needs the program to start after `--`"))?;
    let values = values(store, delegated, env, program).await?;
    let failure = std::process::Command::new(program)
        .args(arguments)
        .envs(&values)
        .exec();
    Err(CmdError::click(format!(
        "{program} could not be started with {}: {failure}",
        values.keys().cloned().collect::<Vec<_>>().join(", ")
    ))
    .stating(crate::cli::entry::error::io_failure_code(failure.kind())))
}
