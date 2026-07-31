use clap::Subcommand;

use super::CmdError;

mod beacon;
mod credential;
mod lifecycle;
mod read;
mod routes;

const DEFAULT_PORT: &str = "8001";
const DEFAULT_CONTEXT: &str = "32768";

fn default_port() -> u16 {
    DEFAULT_PORT.parse().expect("static inference port")
}

fn default_context() -> u64 {
    DEFAULT_CONTEXT.parse().expect("static inference context")
}

fn default_log_lines() -> usize {
    usize::from(u8::MAX)
}

#[derive(Subcommand, Debug)]
pub enum InferenceCommands {
    /// Inspect the target and persist an immutable, registry-bound plan.
    Plan {
        name: String,
        #[arg(long)]
        host: String,
        /// Digest-pinned vLLM image (`repository@sha256:digest`).
        #[arg(long)]
        image: String,
        #[arg(long)]
        model: String,
        /// Immutable Hugging Face commit SHA.
        #[arg(long)]
        revision: String,
        #[arg(long, default_value_t = default_port())]
        port: u16,
        #[arg(long, default_value_t = default_context())]
        max_model_len: u64,
        #[arg(long)]
        json: bool,
    },
    /// Execute one persisted plan if its registry precondition still matches.
    Apply {
        plan_id: String,
        #[arg(long)]
        json: bool,
    },
    /// List declared inference deployments without contacting hosts.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Read one deployment's state from the latest host beacon.
    Status {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Read one deployment's systemd journal over the managed host channel.
    Logs {
        name: String,
        #[arg(long, default_value_t = default_log_lines())]
        lines: usize,
        #[arg(long)]
        json: bool,
    },
    /// Inspect runtime, GPU, endpoint and authentication.
    Doctor {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Send one minimal authenticated OpenAI-compatible completion.
    Verify {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Atomically update a logical route.
    Route {
        #[command(subcommand)]
        command: RouteCommands,
    },
    /// Reinstall the previous deployment generation.
    Rollback {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop and forget a deployment; model cache is retained by default.
    Retire {
        name: String,
        #[arg(long)]
        purge_cache: bool,
        #[arg(long)]
        json: bool,
    },
    /// Create the central local-inference bearer in Skarbiec.
    InitCredential {
        #[arg(long)]
        json: bool,
    },
    /// Emit this host's inference beacon fragment.
    #[command(hide = true)]
    Beacon,
}

#[derive(Subcommand, Debug)]
pub enum RouteCommands {
    Set {
        alias: String,
        #[arg(long)]
        to: String,
        /// Registered host running Brama. Required on the first managed route.
        #[arg(long)]
        gateway: Option<String>,
        /// Ordered fallback routes, attempted after the primary destination.
        #[arg(long)]
        fallback: Vec<String>,
        /// Required compare-and-swap precondition.
        #[arg(long)]
        expected: String,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: InferenceCommands) -> Result<(), CmdError> {
    match command {
        InferenceCommands::Plan {
            name,
            host,
            image,
            model,
            revision,
            port,
            max_model_len,
            json,
        } => {
            lifecycle::plan(lifecycle::PlanOptions {
                name,
                host,
                image,
                model,
                revision,
                port,
                max_model_len,
                json,
            })
            .await
        }
        InferenceCommands::Apply { plan_id, json } => lifecycle::apply(&plan_id, json).await,
        InferenceCommands::List { json } => read::list(json).await,
        InferenceCommands::Status { name, json } => read::status(&name, json).await,
        InferenceCommands::Logs { name, lines, json } => read::logs(&name, lines, json).await,
        InferenceCommands::Doctor { name, json } => read::doctor(&name, json).await,
        InferenceCommands::Verify { name, json } => read::verify(&name, json).await,
        InferenceCommands::Route {
            command:
                RouteCommands::Set {
                    alias,
                    to,
                    expected,
                    gateway,
                    fallback,
                    json,
                },
        } => routes::set(&alias, &to, &expected, gateway.as_deref(), &fallback, json).await,
        InferenceCommands::Rollback { name, json } => lifecycle::rollback(&name, json).await,
        InferenceCommands::Retire {
            name,
            purge_cache,
            json,
        } => lifecycle::retire(&name, purge_cache, json).await,
        InferenceCommands::InitCredential { json } => credential::init(json).await,
        InferenceCommands::Beacon => beacon::local(),
    }
}
