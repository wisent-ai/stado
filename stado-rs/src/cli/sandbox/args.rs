//! The arguments each `stado sandbox` verb takes, as clap reads them.

use clap::{Args, ValueEnum};

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
    /// Start the command in the background and print its process id at
    /// once, for `stado sandbox process status` and `process stop`.
    #[arg(long)]
    pub detach: bool,
    /// The command line, as one argument, run by the sandbox's shell.
    pub command: String,
}

#[derive(Args, Debug)]
pub struct ProcessArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// The process id `stado sandbox exec --detach` printed.
    pub process: i64,
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
