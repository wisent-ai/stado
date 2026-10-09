//! Where the queue-work verbs and the local worker land.

use crate::cli::entry::spec::root::work::{QualityCommands, WorkCommands};
use crate::cli::hosts::machine;
use crate::cli::reporting::{results, status};
use crate::cli::work::cancel;
use crate::cli::*;

pub(crate) async fn dispatch(command: WorkCommands) -> Result<(), CmdError> {
    match command {
        WorkCommands::Submit(args) => submit::run(&args).await,
        WorkCommands::Status { filter_id, json } => status::run(filter_id.as_deref(), json).await,
        WorkCommands::Cancel {
            job_id,
            queued,
            terminate,
        } => cancel::run(job_id.as_deref(), queued, terminate).await,
        WorkCommands::Job(sub) => job::dispatch(sub).await,
        WorkCommands::Results { job_id, output_dir } => results::run(&job_id, &output_dir).await,
        WorkCommands::Quality(sub) => match sub {
            QualityCommands::Format { root } => crate::cli::quality::format(root.as_deref()).await,
            QualityCommands::Check { root } => crate::cli::quality::check(root.as_deref()).await,
        },
        WorkCommands::Mcp => tokio::task::spawn_blocking(|| {
            let stdin = std::io::stdin();
            let mut stdout = std::io::stdout();
            crate::mcp::serve(stdin.lock(), &mut stdout);
        })
        .await
        // The blocking task can only fail by panicking or being cancelled at
        // shutdown: the server stopped serving, its outage.
        .map_err(|error| CmdError::unreachable(format!("the MCP server stopped: {error}"))),
        WorkCommands::Machine(sub) => match sub {
            MachineCommands::Submit { request_file } => machine::submit(&request_file).await,
            MachineCommands::Status { job_id, until } => {
                machine::status(&job_id, matches!(until, Some(StatusHold::Terminal))).await
            }
            MachineCommands::Logs {
                job_id,
                cursor,
                limit,
            } => machine::logs(&job_id, cursor, limit).await,
            MachineCommands::Cancel { job_id } => machine::cancel(&job_id).await,
            MachineCommands::Artifacts { job_id, output_dir } => {
                machine::artifacts(&job_id, &output_dir).await
            }
        },
    }
}
