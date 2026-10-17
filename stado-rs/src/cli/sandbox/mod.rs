//! `stado sandbox` — the Boat sandboxes this deployment's key can reach, as
//! an agent or a script drives them: list and show, create and rename, run a
//! command, read, write and download files, hand a coding agent a prompt and
//! follow it, read the sandbox's events, add an SSH key, fork, stop, resume
//! and delete.
//!
//! The scheduler rents a sandbox per job and owns its whole life; these verbs
//! are the same Boat Public API v1 calls handed to a caller directly, through
//! the one client the provider builds (`BoxProvider::from_env`). So the key
//! is the Skarbiec item that plays `cloud-boat`, `boat.api_url` names another
//! API base, and a sandbox created here lives `boat.ttl_seconds` unless the
//! call states its own lifetime. Every answer is printed as JSON; a refusal
//! Boat gave keeps its status, code and request id in the error.

mod agent;
mod verbs;

use clap::{Args, Subcommand, ValueEnum};

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
    /// Rename a sandbox, or change or clear when it stops by itself.
    Update(UpdateArgs),
    /// Hand the coding agent in a sandbox a prompt and print the prompt run.
    Prompt(PromptArgs),
    /// Print a prompt run's status.
    PromptStatus(PromptRunId),
    /// Interrupt every conversation running in a sandbox.
    Interrupt(SandboxId),
    /// Print one page of a sandbox's events and the cursor of the next.
    Events(EventsArgs),
    /// Allow an OpenSSH public key into a sandbox.
    SshKey(SshKeyArgs),
    /// Download a file or folder from a sandbox into a new local file.
    Download(DownloadArgs),
    /// Give a port inside a sandbox a stable public HTTPS URL.
    Host(HostArgs),
    /// Save, list, show and delete named snapshots: a sandbox's disk kept
    /// under a name, from which `create --from` starts new sandboxes.
    #[command(subcommand)]
    Snapshot(SnapshotCommands),
}

#[derive(Subcommand)]
pub enum SnapshotCommands {
    /// Freeze a sandbox's disk under a name; Boat saves it in the background.
    Save(SnapshotSaveArgs),
    /// List the named snapshots, newest first, with the account's allowance.
    List,
    /// Show one named snapshot and its status.
    Show(SnapshotName),
    /// Delete a named snapshot for good.
    Delete(SnapshotName),
}

#[derive(Args, Debug)]
pub struct SnapshotSaveArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// The name to save under; saving an existing name replaces it.
    pub name: String,
}

#[derive(Args, Debug)]
pub struct SnapshotName {
    /// The named snapshot's name.
    pub name: String,
}

#[derive(Args, Debug)]
pub struct HostArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// The port the service listens on inside the sandbox.
    pub port: u16,
    /// Answer a URL anyone can open, not one gated by its token.
    #[arg(long)]
    pub public: bool,
    /// The service listens on the loopback address only.
    #[arg(long)]
    pub localhost: bool,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// The sandbox's new name.
    #[arg(long)]
    pub name: Option<String>,
    /// Stop the sandbox by itself after this many seconds; a value that is
    /// not positive is refused.
    #[arg(long, conflicts_with = "no_auto_stop")]
    pub ttl_seconds: Option<i64>,
    /// Never stop the sandbox by itself.
    #[arg(long)]
    pub no_auto_stop: bool,
}

#[derive(Args, Debug)]
pub struct PromptArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// What the coding agent is asked to do.
    pub prompt: String,
    /// The agent provider Boat runs the prompt with, as Boat's API names it.
    #[arg(long)]
    pub provider: String,
    /// The provider's model; without it the request names none.
    #[arg(long)]
    pub model: Option<String>,
    /// The reasoning effort; without it the request names none.
    #[arg(long)]
    pub reasoning_effort: Option<String>,
}

#[derive(Args, Debug)]
pub struct PromptRunId {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// The prompt id `stado sandbox prompt` printed.
    pub prompt_id: String,
}

/// The order Boat lists events in; each value is the word Boat's API takes.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum EventOrder {
    Asc,
    Desc,
}

#[derive(Args, Debug)]
pub struct EventsArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// Oldest or newest event first.
    #[arg(long, value_enum)]
    pub sort: EventOrder,
    /// The cursor a previous page printed as nextCursor; without it the
    /// first page.
    #[arg(long)]
    pub cursor: Option<String>,
    /// Only events of this type, as Boat's API names it; without it every
    /// type.
    #[arg(long = "type")]
    pub event_type: Option<String>,
}

#[derive(Args, Debug)]
pub struct SshKeyArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// A local OpenSSH public key file (ssh-...).
    #[arg(long)]
    pub public_key_file: std::path::PathBuf,
}

#[derive(Args, Debug)]
pub struct DownloadArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// Path of the file or folder in the sandbox.
    pub path: String,
    /// The local file the download is written to; one that exists is
    /// refused, never overwritten.
    #[arg(long)]
    pub to: std::path::PathBuf,
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
    /// Start from this named snapshot instead of a fresh disk.
    #[arg(long)]
    pub from: Option<String>,
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
        SandboxCommands::Update(args) => agent::update(&provider, &args).await,
        SandboxCommands::Prompt(args) => agent::prompt(&provider, &args).await,
        SandboxCommands::PromptStatus(run) => agent::prompt_status(&provider, &run).await,
        SandboxCommands::Interrupt(sandbox) => agent::interrupt(&provider, &sandbox.id).await,
        SandboxCommands::Events(args) => agent::events(&provider, &args).await,
        SandboxCommands::SshKey(args) => agent::ssh_key(&provider, &args).await,
        SandboxCommands::Download(args) => agent::download(&provider, &args).await,
        SandboxCommands::Host(args) => agent::host(&provider, &args).await,
        SandboxCommands::Snapshot(sub) => match sub {
            SnapshotCommands::Save(args) => agent::snapshot_save(&provider, &args).await,
            SnapshotCommands::List => agent::snapshot_list(&provider).await,
            SnapshotCommands::Show(named) => agent::snapshot_show(&provider, &named.name).await,
            SnapshotCommands::Delete(named) => agent::snapshot_delete(&provider, &named.name).await,
        },
    }
}
