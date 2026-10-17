//! The `stado sandbox` verbs that read what ran in a sandbox: its agent
//! conversations, the history of its commands and other processes, one
//! command and its stored output, its processes, and the AI requests its
//! agents made through Boat's proxy, which can be blocked. Boat keeps the
//! history and its output, so they are read also while the sandbox is
//! stopped.

use clap::{Args, ValueEnum};
use serde_json::Value;

use crate::providers::BoxProvider;

use super::agent::stated;
use super::verbs::{failure, print};
use super::CmdError;

/// A kind of run Boat's command history keeps; each value is the word
/// Boat's API takes, and `commands` stands for the first three.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum HistoryMode {
    Sync,
    Stream,
    Detached,
    Process,
    Commands,
}

/// Which stored output of a command is read.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// How Boat answers the bytes read; base64 keeps them exact.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OutputEncoding {
    Utf8,
    Base64,
}

/// Whether a sandbox's requests through Boat's AI proxy are let through.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum AiAccess {
    Allowed,
    Blocked,
}

#[derive(Args, Debug)]
pub struct AiUsageArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// Days of history, today included; without it Boat's own span.
    #[arg(long)]
    pub days: Option<u32>,
}

#[derive(Args, Debug)]
pub struct AiAccessArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// Let its AI requests through, or refuse them.
    #[arg(long, value_enum)]
    pub state: AiAccess,
}

#[derive(Args, Debug)]
pub struct HistoryArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// Only these kinds of run, comma-separated; without it every kind.
    #[arg(long, value_enum, value_delimiter = ',')]
    pub mode: Vec<HistoryMode>,
    /// Only commands whose command line contains this text, in any case.
    #[arg(long)]
    pub query: Option<String>,
    /// How many commands one page holds; without it Boat's own page size.
    #[arg(long)]
    pub limit: Option<u32>,
    /// The nextCursor a previous page printed; without it the first page.
    #[arg(long)]
    pub cursor: Option<String>,
}

#[derive(Args, Debug)]
pub struct CommandRef {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// The command id the history printed.
    pub command: String,
}

#[derive(Args, Debug)]
pub struct LogsArgs {
    /// The sandbox id Boat answered at create, fork or list.
    pub id: String,
    /// The command id the history printed.
    pub command: String,
    /// The output to read.
    #[arg(long, value_enum)]
    pub stream: OutputStream,
    /// How Boat answers the bytes.
    #[arg(long, value_enum)]
    pub encoding: OutputEncoding,
    /// The byte to start at, as a previous read printed it in nextOffset;
    /// without it the read starts where Boat starts it.
    #[arg(long)]
    pub offset: Option<u64>,
}

/// The word Boat's API takes for a value of one of these options.
fn word<T: ValueEnum + std::fmt::Debug>(value: &T, option: &str) -> Result<String, CmdError> {
    match value.to_possible_value() {
        Some(possible) => Ok(possible.get_name().to_string()),
        None => Err(CmdError::usage(format!(
            "{option} {value:?} names no value Boat's API takes"
        ))),
    }
}

pub(super) async fn conversations(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    let answer = provider.client.conversations(id).await.map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn history(provider: &BoxProvider, args: &HistoryArgs) -> Result<(), CmdError> {
    let modes = args
        .mode
        .iter()
        .map(|mode| word(mode, "--mode"))
        .collect::<Result<Vec<_>, _>>()?
        .join(",");
    let query = stated(args.query.as_deref(), "--query")?;
    let cursor = stated(args.cursor.as_deref(), "--cursor")?;
    let answer = provider
        .client
        .command_history(&args.id, &modes, query, args.limit, cursor)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn command(provider: &BoxProvider, args: &CommandRef) -> Result<(), CmdError> {
    let answer = provider
        .client
        .command_info(&args.id, &args.command)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn logs(provider: &BoxProvider, args: &LogsArgs) -> Result<(), CmdError> {
    let stream = word(&args.stream, "--stream")?;
    let encoding = word(&args.encoding, "--encoding")?;
    let answer = provider
        .client
        .command_logs(&args.id, &args.command, &stream, args.offset, &encoding)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn processes(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    let answer = provider.client.processes(id).await.map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn ai_usage(provider: &BoxProvider, args: &AiUsageArgs) -> Result<(), CmdError> {
    let answer = provider
        .client
        .ai_usage(&args.id, args.days)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn ai_access(provider: &BoxProvider, args: &AiAccessArgs) -> Result<(), CmdError> {
    let blocked = match args.state {
        AiAccess::Allowed => false,
        AiAccess::Blocked => true,
    };
    let answer = provider
        .client
        .set_ai_access(&args.id, blocked)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}
