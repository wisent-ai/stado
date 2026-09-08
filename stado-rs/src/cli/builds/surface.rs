//! The `stado builds` command surface: the subcommands clap parses and the
//! dispatch that hands each one to the component that answers it.

use clap::Subcommand;

use crate::cli::builds::declaration::{add, edit, remove, set_enabled, RecipeEdit};
use crate::cli::builds::jobs::run_now;
use crate::cli::builds::report::{list, status};
use crate::cli::CmdError;

#[derive(Subcommand)]
pub enum BuildsCommands {
    /// List every build recipe in the registry.
    List {
        /// Emit the machine-readable recipe array.
        #[arg(long)]
        json: bool,
    },
    /// Add a build recipe. Recipes start disabled; enable one explicitly.
    Add {
        /// Unique kebab-case recipe name.
        #[arg(long)]
        name: String,
        /// HTTPS clone URL of the repository to build.
        #[arg(long)]
        repo: String,
        /// Branch the poller watches.
        #[arg(long)]
        branch: String,
        /// Single POSIX sh build command run in the checkout.
        #[arg(long)]
        command: String,
        /// Path in the checkout to upload as a build artifact (repeatable).
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<String>,
        /// Release platform to build for, e.g. `darwin-arm64` (repeatable,
        /// at least one). Each platform gets its own build job, claimed only
        /// by a worker that is actually that platform.
        #[arg(long = "platform", required = true)]
        platforms: Vec<String>,
        /// Declare a successful run's tag version on every registry host of
        /// that platform. Never promotes a signed release.
        #[arg(long)]
        auto_declare: bool,
        /// Poll cadence in seconds (default 300).
        #[arg(long, default_value_t = 300)]
        interval_seconds: u64,
        /// Emit the created recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Change a recipe's source or build definition in place. Every flag is
    /// optional and a flag not given leaves its field alone; `--artifact` and
    /// `--platform`, when given at all, REPLACE the recorded list. `enabled`
    /// is not editable here: `enable` and `disable` own it.
    Edit {
        name: String,
        /// HTTPS clone URL of the repository to build. Changing it clears the
        /// last seen ref and the recorded runs.
        #[arg(long)]
        repo: Option<String>,
        /// Branch the poller watches. Changing it clears the last seen ref
        /// and the recorded runs.
        #[arg(long)]
        branch: Option<String>,
        /// Single POSIX sh build command run in the checkout.
        #[arg(long)]
        command: Option<String>,
        /// Path in the checkout to upload as a build artifact (repeatable);
        /// the paths given replace the recorded ones.
        #[arg(long = "artifact")]
        artifacts: Vec<String>,
        /// Release platform to build for (repeatable); the platforms given
        /// replace the recorded ones. A newly named platform simply has no
        /// run yet.
        #[arg(long = "platform")]
        platforms: Vec<String>,
        /// Declare a successful run's tag version on every registry host of
        /// that platform. Never promotes a signed release.
        #[arg(long = "auto-declare", overrides_with = "no_auto_declare")]
        auto_declare: bool,
        /// Stop declaring versions from successful runs.
        #[arg(long = "no-auto-declare", overrides_with = "auto_declare")]
        no_auto_declare: bool,
        /// Poll cadence in seconds.
        #[arg(long)]
        interval_seconds: Option<u64>,
        /// Emit the updated recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove a build recipe.
    Remove {
        name: String,
        /// Emit `{"name": ..., "removed": true}` as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Enable a recipe: the control-plane poller starts building it.
    Enable {
        name: String,
        /// Emit the updated recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Disable a recipe without deleting it.
    Disable {
        name: String,
        /// Emit the updated recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Enqueue one build job per platform for a recipe now, ignoring the
    /// poll cadence.
    Run {
        name: String,
        /// Caller-retained token; reuse it to recover the same durable run.
        #[arg(long)]
        run_id: String,
        /// Emit the submitted job ids and updated recipe as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show one recipe and the state of its per-platform build jobs.
    Status {
        name: String,
        /// Emit the recipe and job states as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(command: BuildsCommands) -> Result<(), CmdError> {
    match command {
        BuildsCommands::List { json } => list(json).await,
        BuildsCommands::Add {
            name,
            repo,
            branch,
            command,
            artifacts,
            platforms,
            auto_declare,
            interval_seconds,
            json,
        } => {
            add(
                &name,
                &repo,
                &branch,
                &command,
                artifacts,
                platforms,
                auto_declare,
                interval_seconds,
                json,
            )
            .await
        }
        BuildsCommands::Edit {
            name,
            repo,
            branch,
            command,
            artifacts,
            platforms,
            auto_declare,
            no_auto_declare,
            interval_seconds,
            json,
        } => {
            edit(
                &name,
                RecipeEdit {
                    repo,
                    branch,
                    command,
                    // An empty repeatable is the flag never given; the
                    // operator cannot ask for an empty list, only for a
                    // different one.
                    artifacts: (!artifacts.is_empty()).then_some(artifacts),
                    platforms: (!platforms.is_empty()).then_some(platforms),
                    // `overrides_with` in both directions leaves at most one
                    // of the pair set, so neither set is "leave it alone".
                    auto_declare: match (auto_declare, no_auto_declare) {
                        (true, false) => Some(true),
                        (false, true) => Some(false),
                        _ => None,
                    },
                    interval_seconds,
                },
                json,
            )
            .await
        }
        BuildsCommands::Remove { name, json } => remove(&name, json).await,
        BuildsCommands::Enable { name, json } => set_enabled(&name, true, json).await,
        BuildsCommands::Disable { name, json } => set_enabled(&name, false, json).await,
        BuildsCommands::Run { name, run_id, json } => run_now(&name, &run_id, json).await,
        BuildsCommands::Status { name, json } => status(&name, json).await,
    }
}
