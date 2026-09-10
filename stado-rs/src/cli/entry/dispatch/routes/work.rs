//! Where the queue-work verbs and the local worker land.

use crate::cli::entry::spec::root::work::WorkCommands;
use crate::cli::hosts::{agent, machine};
use crate::cli::reporting::{results, status};
use crate::cli::work::cancel;
use crate::cli::*;

pub(crate) async fn dispatch(command: WorkCommands) -> Result<(), CmdError> {
    match command {
        WorkCommands::Submit(args) => submit::run(&args).await,
        WorkCommands::Status { filter_id } => status::run(filter_id.as_deref()).await,
        WorkCommands::Cancel { job_id, terminate } => cancel::run(&job_id, terminate).await,
        WorkCommands::Job(sub) => job::dispatch(sub).await,
        WorkCommands::Results { job_id, output_dir } => results::run(&job_id, &output_dir).await,
        WorkCommands::Machine(sub) => match sub {
            MachineCommands::Submit { request_file } => machine::submit(&request_file).await,
            MachineCommands::Status { job_id } => machine::status(&job_id).await,
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
        WorkCommands::Agent {
            gpu_type,
            target,
            auto,
            idle_shutdown,
            kind,
            vast_auto_list,
            vast_price_gpu,
            vast_max_duration_s,
        } => {
            agent::run(
                gpu_type,
                target,
                auto,
                idle_shutdown,
                kind,
                vast_auto_list,
                vast_price_gpu,
                vast_max_duration_s,
            )
            .await
        }
    }
}
