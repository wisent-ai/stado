//! `stado sandbox` — the Boat sandboxes this deployment's key can reach, as
//! an agent or a script drives them: list and show, create and rename, run a
//! command and follow or stop one run in the background, read, write and
//! download files, hand a coding agent a prompt and follow it, read the
//! sandbox's events and usage, its conversations, the history of its
//! commands with their stored output and its processes, add an SSH key,
//! give a port a URL, keep the disk as a named snapshot, fork, stop (or
//! force a stop whose save fails), power-cycle one that stopped answering,
//! share one with its organization, resume and delete, and register the
//! webhooks Boat posts every sandbox's lifecycle events to.
//!
//! The scheduler rents a sandbox per job and owns its whole life; these verbs
//! are the same Boat Public API v1 calls handed to a caller directly, through
//! the one client the provider builds (`BoxProvider::from_env`). So the key
//! is the Skarbiec item that plays `cloud-boat`, `boat.api_url` names another
//! API base, and a sandbox created here lives `boat.ttl_seconds` unless the
//! call states its own lifetime. Every answer is printed as JSON; a refusal
//! Boat gave keeps its status, code and request id in the error.

mod agent;
mod args;
mod environments;
mod history;
mod snapshots;
mod verbs;
mod webhooks;

use clap::Subcommand;

pub use agent::FileUrlArgs;
pub use args::*;
pub use history::{CommandRef, HistoryArgs, LogsArgs};
pub use snapshots::ShareRef;
pub use webhooks::WebhookCommands;

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
    /// With --detach it starts in the background and its process id is
    /// printed.
    Exec(ExecArgs),
    /// Follow or stop a command started with `exec --detach`.
    #[command(subcommand)]
    Process(ProcessCommands),
    /// Print what a sandbox has run and cost.
    Usage(SandboxId),
    /// Read a file from a sandbox and print Boat's answer.
    Read(ReadArgs),
    /// Write a local file into a sandbox.
    Write(WriteArgs),
    /// Create a new sandbox from an existing one's disk.
    Fork(EnvArgs),
    /// Stop a sandbox; Boat saves its disk first and keeps it for resume or
    /// fork. With --force it stops even though that save fails.
    Stop(StopArgs),
    /// Power-cycle the machine of a sandbox that stopped answering; its
    /// disk is not touched.
    Recover(SandboxId),
    /// Let every member of the paying organization use a sandbox; the
    /// creator's logins are wiped from it at its next start. One-way.
    Share(SandboxId),
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
    /// Give the running turn of a sandbox's coding agent an extra
    /// instruction; it keeps what it was doing.
    Steer(SteerArgs),
    /// Print the URL that streams a sandbox's desktop; whoever has it sees
    /// the desktop.
    Desktop(SandboxId),
    /// Interrupt every conversation running in a sandbox.
    Interrupt(SandboxId),
    /// Print one page of a sandbox's events and the cursor of the next.
    Events(EventsArgs),
    /// Allow an OpenSSH public key into a sandbox.
    SshKey(SshKeyArgs),
    /// Download a file or folder from a sandbox into a new local file.
    Download(DownloadArgs),
    /// Print a short-lived URL the sandbox serves for one file: a GET of it
    /// downloads the file, a PUT of the bytes writes it, with no size limit
    /// and no key. The URL is a secret until it expires.
    FileUrl(FileUrlArgs),
    /// Give a port inside a sandbox a stable public HTTPS URL.
    Host(HostArgs),
    /// Save, list, show and delete named snapshots: a sandbox's disk kept
    /// under a name, from which `create --from` starts new sandboxes.
    #[command(subcommand)]
    Snapshot(SnapshotCommands),
    /// Manage environments: the variables, secret files and repositories
    /// every new sandbox started with one gets.
    #[command(subcommand)]
    Environment(EnvironmentCommands),
    /// Print a sandbox's agent conversations, most recently prompted first.
    Conversations(SandboxId),
    /// Print one page of the commands and other processes that ran in a
    /// sandbox, newest first; also while it is stopped.
    History(HistoryArgs),
    /// Print one command of a sandbox's history.
    Command(CommandRef),
    /// Print a command's stored output from an offset; also while the
    /// sandbox is stopped.
    Logs(LogsArgs),
    /// Print the processes of a sandbox, busiest first, or the last sample
    /// Boat kept of a stopped one.
    Processes(SandboxId),
    /// Manage the account's webhooks: the HTTPS endpoints Boat posts sandbox
    /// lifecycle events to, and their signing secrets.
    #[command(subcommand)]
    Webhook(WebhookCommands),
}

#[derive(Subcommand)]
pub enum EnvironmentCommands {
    /// List the environments.
    List,
    /// Create an empty environment.
    Create(EnvironmentName),
    /// Delete an environment.
    Delete(EnvironmentId),
    /// Set a variable; its value is the one line on standard input.
    SetVar(EnvironmentKey),
    /// Remove a variable.
    RmVar(EnvironmentKey),
    /// Set a secret file from a local file's text.
    SetFile(SecretFileArgs),
    /// Remove a secret file.
    RmFile(SecretPath),
    /// Clone a GitHub repository into every new sandbox.
    AddRepo(RepositoryArgs),
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
    /// List every snapshot of one sandbox's disk.
    History(SandboxId),
    /// Show the snapshot a sandbox's resume and fork start from.
    Latest(SandboxId),
    /// List the files and folders a snapshot holds, with their sizes.
    Tree(SnapshotId),
    /// Write a file or folder (as a tar) of a snapshot into a new local
    /// file; the sandbox need not run.
    Get(SnapshotGetArgs),
    /// List a named snapshot's share codes, active and revoked, without the
    /// codes themselves.
    Shares(SnapshotName),
    /// Make a secret code with which any Boat account starts a sandbox from
    /// a named snapshot (`create --from <code>`); it is printed only now, and
    /// everything on the disk is shared.
    Share(SnapshotName),
    /// Revoke a share code; sandboxes already made from it keep their copy.
    Unshare(ShareRef),
}

#[derive(Subcommand)]
pub enum ProcessCommands {
    /// Whether the command runs, its exit code and its recent log.
    Status(ProcessArgs),
    /// Stop the command.
    Stop(ProcessArgs),
}

pub async fn dispatch(command: SandboxCommands) -> Result<(), CmdError> {
    let provider = verbs::provider()?;
    match command {
        SandboxCommands::List => verbs::list(&provider).await,
        SandboxCommands::Show(sandbox) => verbs::show(&provider, &sandbox.id).await,
        SandboxCommands::Create(args) => verbs::create(&provider, &args).await,
        SandboxCommands::Exec(args) => verbs::exec(&provider, &args).await,
        SandboxCommands::Process(sub) => match sub {
            ProcessCommands::Status(args) => agent::process_status(&provider, &args).await,
            ProcessCommands::Stop(args) => agent::process_stop(&provider, &args).await,
        },
        SandboxCommands::Usage(sandbox) => agent::usage(&provider, &sandbox.id).await,
        SandboxCommands::Conversations(sandbox) => {
            history::conversations(&provider, &sandbox.id).await
        }
        SandboxCommands::History(args) => history::history(&provider, &args).await,
        SandboxCommands::Command(args) => history::command(&provider, &args).await,
        SandboxCommands::Logs(args) => history::logs(&provider, &args).await,
        SandboxCommands::Processes(sandbox) => history::processes(&provider, &sandbox.id).await,
        SandboxCommands::FileUrl(args) => agent::file_url(&provider, &args).await,
        SandboxCommands::Read(args) => verbs::read(&provider, &args).await,
        SandboxCommands::Write(args) => verbs::write(&provider, &args).await,
        SandboxCommands::Fork(args) => verbs::fork(&provider, &args).await,
        SandboxCommands::Stop(args) => verbs::stop(&provider, &args).await,
        SandboxCommands::Recover(sandbox) => verbs::recover(&provider, &sandbox.id).await,
        SandboxCommands::Share(sandbox) => verbs::share(&provider, &sandbox.id).await,
        SandboxCommands::Resume(args) => verbs::resume(&provider, &args).await,
        SandboxCommands::Delete(sandbox) => verbs::delete(&provider, &sandbox.id).await,
        SandboxCommands::Update(args) => agent::update(&provider, &args).await,
        SandboxCommands::Prompt(args) => agent::prompt(&provider, &args).await,
        SandboxCommands::PromptStatus(run) => agent::prompt_status(&provider, &run).await,
        SandboxCommands::Steer(args) => agent::steer(&provider, &args).await,
        SandboxCommands::Desktop(sandbox) => agent::desktop(&provider, &sandbox.id).await,
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
            SnapshotCommands::History(sandbox) => snapshots::history(&provider, &sandbox.id).await,
            SnapshotCommands::Latest(sandbox) => snapshots::latest(&provider, &sandbox.id).await,
            SnapshotCommands::Tree(snapshot) => {
                snapshots::tree(&provider, &snapshot.snapshot).await
            }
            SnapshotCommands::Get(args) => snapshots::get(&provider, &args).await,
            SnapshotCommands::Shares(named) => snapshots::shares(&provider, &named.name).await,
            SnapshotCommands::Share(named) => snapshots::share(&provider, &named.name).await,
            SnapshotCommands::Unshare(args) => snapshots::unshare(&provider, &args).await,
        },
        SandboxCommands::Environment(sub) => match sub {
            EnvironmentCommands::List => environments::list(&provider).await,
            EnvironmentCommands::Create(named) => {
                environments::create(&provider, &named.name).await
            }
            EnvironmentCommands::Delete(args) => environments::delete(&provider, &args).await,
            EnvironmentCommands::SetVar(args) => environments::set_var(&provider, &args).await,
            EnvironmentCommands::RmVar(args) => environments::remove_var(&provider, &args).await,
            EnvironmentCommands::SetFile(args) => environments::set_file(&provider, &args).await,
            EnvironmentCommands::RmFile(args) => environments::remove_file(&provider, &args).await,
            EnvironmentCommands::AddRepo(args) => environments::add_repo(&provider, &args).await,
        },
        SandboxCommands::Webhook(sub) => webhooks::dispatch(&provider, sub).await,
    }
}
