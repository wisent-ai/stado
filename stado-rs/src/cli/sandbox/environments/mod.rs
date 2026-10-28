//! The `stado sandbox environment` verbs: what every new sandbox started
//! with an environment gets — variables, secret files and GitHub
//! repositories. Each change mints a new environment version in Boat; a
//! sandbox already running stays on its version. A variable's value is read
//! from standard input and a secret file's text from a local file, so no
//! secret stands on a command line.

use std::io::Read;

use clap::{Args, Subcommand};
use serde_json::Value;

use crate::providers::BoxProvider;

use super::agent::stated;
use super::verbs::{failure, print};
use super::{
    CmdError, EnvironmentId, EnvironmentKey, EnvironmentName, RepositoryArgs, SecretFileArgs,
    SecretPath,
};

#[derive(Subcommand)]
pub enum EnvironmentCommands {
    /// List the environments.
    List,
    /// Create an empty environment.
    Create(EnvironmentName),
    /// Delete an environment.
    Delete(EnvironmentId),
    /// Set or remove an environment's variables.
    #[command(subcommand)]
    Var(VarCommands),
    /// Set or remove an environment's secret files.
    #[command(subcommand)]
    File(FileCommands),
    /// List the GitHub repositories Boat can clone, or clone one into every
    /// new sandbox.
    #[command(subcommand)]
    Repo(RepoCommands),
}

#[derive(Subcommand)]
pub enum VarCommands {
    /// Set a variable; its value is the one line on standard input.
    Set(EnvironmentKey),
    /// Remove a variable.
    Remove(EnvironmentKey),
}

#[derive(Subcommand)]
pub enum FileCommands {
    /// Set a secret file from a local file's text.
    Set(SecretFileArgs),
    /// Remove a secret file.
    Remove(SecretPath),
}

#[derive(Subcommand)]
pub enum RepoCommands {
    /// List the GitHub repositories Boat can clone, with the id `repo add`
    /// takes, and the ones already selected.
    List(RepositoryListArgs),
    /// Clone a GitHub repository into every new sandbox.
    Add(RepositoryArgs),
}

/// Run one `stado sandbox environment` command against Boat.
pub(super) async fn dispatch(
    provider: &BoxProvider,
    command: EnvironmentCommands,
) -> Result<(), CmdError> {
    match command {
        EnvironmentCommands::List => list(provider).await,
        EnvironmentCommands::Create(named) => create(provider, &named.name).await,
        EnvironmentCommands::Delete(args) => delete(provider, &args).await,
        EnvironmentCommands::Var(VarCommands::Set(args)) => set_var(provider, &args).await,
        EnvironmentCommands::Var(VarCommands::Remove(args)) => remove_var(provider, &args).await,
        EnvironmentCommands::File(FileCommands::Set(args)) => set_file(provider, &args).await,
        EnvironmentCommands::File(FileCommands::Remove(args)) => remove_file(provider, &args).await,
        EnvironmentCommands::Repo(RepoCommands::List(args)) => repositories(provider, &args).await,
        EnvironmentCommands::Repo(RepoCommands::Add(args)) => add_repo(provider, &args).await,
    }
}

#[derive(Args, Debug)]
pub struct RepositoryListArgs {
    /// Only repositories whose name contains this text, in any case.
    #[arg(long)]
    pub query: Option<String>,
    /// Read the repositories from GitHub again before answering.
    #[arg(long)]
    pub sync: bool,
    /// The cursor a previous page printed; without it the first page.
    #[arg(long)]
    pub cursor: Option<String>,
}

pub(super) async fn repositories(
    provider: &BoxProvider,
    args: &RepositoryListArgs,
) -> Result<(), CmdError> {
    let query = stated(args.query.as_deref(), "--query")?;
    let cursor = stated(args.cursor.as_deref(), "--cursor")?;
    let answer = provider
        .client
        .repositories(query, args.sync, cursor)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn list(provider: &BoxProvider) -> Result<(), CmdError> {
    let answer = provider.client.environments().await.map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn create(provider: &BoxProvider, name: &str) -> Result<(), CmdError> {
    let answer = provider
        .client
        .create_environment(name)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn delete(provider: &BoxProvider, args: &EnvironmentId) -> Result<(), CmdError> {
    let answer = provider
        .client
        .delete_environment(&args.environment)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

/// The one line standard input holds, without its line ending; input that
/// holds no line, an empty one or more than one is refused naming the key.
fn standard_input_value(key: &str) -> Result<String, CmdError> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| {
            CmdError::click(format!(
                "cannot read the value of {key} from standard input: {error}"
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    let mut lines = text.lines();
    match (lines.next(), lines.next()) {
        (Some(value), None) if !value.is_empty() => Ok(value.to_string()),
        (Some(_), Some(_)) => Err(CmdError::usage(format!(
            "standard input holds more than one line for {key}; a variable's value is one line"
        ))),
        _ => Err(CmdError::usage(format!(
            "standard input holds no value for {key}"
        ))),
    }
}

pub(super) async fn set_var(provider: &BoxProvider, args: &EnvironmentKey) -> Result<(), CmdError> {
    let value = standard_input_value(&args.key)?;
    let answer = provider
        .client
        .set_environment_variable(&args.environment, &args.key, &value)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn remove_var(
    provider: &BoxProvider,
    args: &EnvironmentKey,
) -> Result<(), CmdError> {
    let answer = provider
        .client
        .remove_environment_variable(&args.environment, &args.key)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn set_file(
    provider: &BoxProvider,
    args: &SecretFileArgs,
) -> Result<(), CmdError> {
    let contents = std::fs::read_to_string(&args.from).map_err(|error| {
        CmdError::click(format!(
            "cannot read {} for the secret file {} of environment {}: {error}",
            args.from.display(),
            args.path,
            args.environment
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let answer = provider
        .client
        .set_environment_secret_file(&args.environment, &args.path, &contents)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn remove_file(provider: &BoxProvider, args: &SecretPath) -> Result<(), CmdError> {
    let answer = provider
        .client
        .remove_environment_secret_file(&args.environment, &args.path)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn add_repo(
    provider: &BoxProvider,
    args: &RepositoryArgs,
) -> Result<(), CmdError> {
    if args.base_branch.as_deref() == Some("") {
        return Err(CmdError::usage("--base-branch is given with no branch"));
    }
    let answer = provider
        .client
        .add_environment_repository(
            &args.environment,
            &args.repository,
            args.base_branch.as_deref(),
        )
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}
