use clap::Subcommand;

use super::CmdError;

mod beacon;
mod credential;
mod lifecycle;
mod process;
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
        /// `exclusive` keeps the GPU reserved; `yieldable` pauses inference
        /// whenever an eligible GPU job is queued and resumes it afterward.
        #[arg(long, default_value = "exclusive", value_parser = ["exclusive", "yieldable"])]
        gpu_mode: String,
        #[arg(long, default_value_t = default_port())]
        port: u16,
        #[arg(long, default_value_t = default_context())]
        max_model_len: u64,
        /// Fixed vLLM KV-cache allocation in GiB; omit to use the image policy.
        #[arg(long)]
        kv_cache_memory_gb: Option<u64>,
        /// Persistent host directory for the Hugging Face model cache.
        #[arg(long)]
        cache_dir: Option<String>,
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
    /// Read systemd logs for a runtime that has not committed its plan.
    PlanLogs {
        plan_id: String,
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
    ///
    /// Without `--from` the call is made on the deployment's own host, which
    /// proves the model answers and nothing about who can reach it. `--from`
    /// sends the same completion from another registry host, so "the model is
    /// up" and "the consumer can reach it" stop being one answer.
    Verify {
        name: String,
        /// Registry host the completion is sent from; default is the deployment's host.
        #[arg(long)]
        from: Option<String>,
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
    /// List GPU compute processes with PID-reuse-safe identities.
    Blockers {
        #[arg(long)]
        host: String,
        #[arg(long)]
        json: bool,
    },
    /// Gracefully stop one exact GPU process; optionally escalate to KILL.
    Release {
        #[arg(long)]
        host: String,
        /// Exact PID:START_TICKS value printed by `blockers`.
        #[arg(long)]
        identity: String,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Stop runtime left by an uncommitted plan and optionally remove its cache.
    Abort {
        plan_id: String,
        #[arg(long)]
        purge_cache: bool,
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
    /// Retire one alias from the route table. Refused unless `--expected`
    /// names the destination it currently has; the gateway stops serving the
    /// alias in the same commit, so remove it only after every consumer has
    /// stopped asking for it.
    Remove {
        alias: String,
        /// Required compare-and-swap precondition: the alias's current destination.
        #[arg(long)]
        expected: String,
        #[arg(long)]
        json: bool,
    },
    /// Per alias, what the registry declares and what the gateway host is
    /// actually serving. Exits non-zero when the two disagree; `--repair`
    /// stages and commits the declaration onto the host.
    Show {
        /// Send the declared table to the gateway host instead of only reporting.
        #[arg(long)]
        repair: bool,
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
            gpu_mode,
            port,
            max_model_len,
            kv_cache_memory_gb,
            cache_dir,
            json,
        } => {
            lifecycle::plan(lifecycle::PlanOptions {
                name,
                host,
                image,
                model,
                revision,
                gpu_mode,
                port,
                max_model_len,
                kv_cache_memory_gb,
                cache_dir,
                json,
            })
            .await
        }
        InferenceCommands::Apply { plan_id, json } => lifecycle::apply(&plan_id, json).await,
        InferenceCommands::List { json } => read::list(json).await,
        InferenceCommands::Status { name, json } => read::status(&name, json).await,
        InferenceCommands::Logs { name, lines, json } => read::logs(&name, lines, json).await,
        InferenceCommands::Doctor { name, json } => read::doctor(&name, json).await,
        InferenceCommands::Verify { name, from, json } => {
            read::verify(&name, from.as_deref(), json).await
        }
        InferenceCommands::PlanLogs {
            plan_id,
            lines,
            json,
        } => read::plan_logs(&plan_id, lines, json).await,
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
        InferenceCommands::Route {
            command:
                RouteCommands::Remove {
                    alias,
                    expected,
                    json,
                },
        } => routes::remove(&alias, &expected, json).await,
        InferenceCommands::Route {
            command: RouteCommands::Show { repair, json },
        } => routes::show(repair, json).await,
        InferenceCommands::Blockers { host, json } => process::blockers(&host, json).await,
        InferenceCommands::Release {
            host,
            identity,
            force,
            json,
        } => process::release(&host, &identity, force, json).await,
        InferenceCommands::Abort {
            plan_id,
            purge_cache,
            json,
        } => lifecycle::abort(&plan_id, purge_cache, json).await,
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
