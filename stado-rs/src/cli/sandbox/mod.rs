//! `stado sandbox` — the Boat sandboxes this deployment's key can reach, as
//! an agent or a script drives them: list and show, create, run a command,
//! read and write a file, fork, stop, resume and delete.
//!
//! The scheduler rents a sandbox per job and owns its whole life; these verbs
//! are the same Boat Public API v1 calls handed to a caller directly, through
//! the one client the provider builds (`BoxProvider::from_env`). So the key
//! is the Skarbiec item that plays `cloud-boat`, `boat.api_url` names another
//! API base, and a sandbox created here lives `boat.ttl_seconds` unless the
//! call states its own lifetime. Every answer is printed as JSON; a refusal
//! Boat gave keeps its status, code and request id in the error.

mod verbs;

use clap::{Args, Subcommand};

use super::CmdError;

#[derive(Subcommand)]
pub enum SandboxCommands {
    /// List every sandbox the key can reach, every page.
    List,
    /// Show one sandbox: its state, address and snapshot status.
    Show(SandboxId),
    /// Create a sandbox and print it.
    Create(CreateArgs),
    /// Run a command in a sandbox and print its exit code, signal and
    /// output; a command that does not succeed fails this one with its code.
    Exec(ExecArgs),
    /// Read a file from a sandbox and print Boat's answer.
    Read(ReadArgs),
    /// Write a local file into a sandbox.
    Write(WriteArgs),
    /// Create a new sandbox from an existing one's disk.
    Fork(EnvArgs),
    /// Stop a sandbox; Boat saves its disk first and keeps it for resume or
    /// fork.
    Stop(SandboxId),
    /// Resume a stopped sandbox from its saved disk.
    Resume(EnvArgs),
    /// Delete a sandbox and its data for good.
    Delete(SandboxId),
}

#[derive(Args, Debug)]
pub struct SandboxId {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
}

#[derive(Args, Debug)]
pub struct CreateArgs {
    /// How long the sandbox lives, in seconds, instead of the deployment's
    /// `boat.ttl_seconds`; a value that is not positive is refused.
    #[arg(long)]
    pub ttl_seconds: Option<i64>,
    /// Start without the account's environment variables.
    #[arg(long)]
    pub no_env: bool,
}

#[derive(Args, Debug)]
pub struct EnvArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// Start without the account's environment variables.
    #[arg(long)]
    pub no_env: bool,
}

#[derive(Args, Debug)]
pub struct ExecArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// Directory in the sandbox the command runs in; without it the request
    /// names none and Boat chooses.
    #[arg(long)]
    pub cwd: Option<String>,
    /// The command line, as one argument, run by the sandbox's shell.
    pub command: String,
}

#[derive(Args, Debug)]
pub struct ReadArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// Path of the file in the sandbox.
    pub path: String,
    /// The encoding Boat answers the content in, as its API names it.
    #[arg(long)]
    pub encoding: String,
}

#[derive(Args, Debug)]
pub struct WriteArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// Path of the file in the sandbox.
    pub path: String,
    /// The local file whose text is written.
    #[arg(long)]
    pub from: std::path::PathBuf,
    /// The encoding the content is sent in, as Boat's API names it.
    #[arg(long)]
    pub encoding: String,
}

pub async fn dispatch(command: SandboxCommands) -> Result<(), CmdError> {
    let provider = verbs::provider()?;
    match command {
        SandboxCommands::List => verbs::list(&provider).await,
        SandboxCommands::Show(sandbox) => verbs::show(&provider, &sandbox.id).await,
        SandboxCommands::Create(args) => verbs::create(&provider, &args).await,
        SandboxCommands::Exec(args) => verbs::exec(&provider, &args).await,
        SandboxCommands::Read(args) => verbs::read(&provider, &args).await,
        SandboxCommands::Write(args) => verbs::write(&provider, &args).await,
        SandboxCommands::Fork(args) => verbs::fork(&provider, &args).await,
        SandboxCommands::Stop(sandbox) => verbs::stop(&provider, &sandbox.id).await,
        SandboxCommands::Resume(args) => verbs::resume(&provider, &args).await,
        SandboxCommands::Delete(sandbox) => verbs::delete(&provider, &sandbox.id).await,
    }
}
