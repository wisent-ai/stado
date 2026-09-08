use serde_json::{Map, Value};

use super::{build_universe, list_universes, state_load, verify, verify_and_retry, CoverageError};
use crate::config;
use crate::models::py_str_repr;
use crate::queue::JobStorage;

// ---------------------------------------------------------------------------
// coverage/cli.py — `stado-coverage` click group
// ---------------------------------------------------------------------------

/// Python `_coerce`: KEY=VALUE -> typed value. Comma-list (stripped,
/// empties dropped), int, or str.
pub fn coerce(value: &str) -> Value {
    if value.contains(',') {
        return Value::Array(
            value
                .split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(|part| Value::from(part.to_string()))
                .collect(),
        );
    }
    match python_int(value) {
        Some(int) => Value::from(int),
        None => Value::from(value),
    }
}

/// Python `int(str)` semantics used by `_coerce`: optional sign, `_`
/// digit separators between digits.
fn python_int(raw: &str) -> Option<i64> {
    let digits = raw.strip_prefix(['+', '-']).unwrap_or(raw);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let cleaned: String = raw.chars().filter(|&c| c != '_').collect();
    cleaned.parse::<i64>().ok()
}

/// Python `_kv_to_kwargs`: repeated `--kv KEY=VALUE` flags -> constructor
/// kwargs. The `Err` string is the click UsageError message.
pub fn kv_to_kwargs(pairs: &[String]) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("--kv expects KEY=VALUE, got {}", py_str_repr(pair)));
        };
        out.insert(key.trim().to_string(), coerce(value.trim()));
    }
    Ok(out)
}

/// Print `value` as Python `json.dumps(value, indent=2)` (insertion
/// order, ensure_ascii) on stdout.
fn print_pretty(value: &Value) {
    let pretty = serde_json::to_string_pretty(value).expect("JSON serialization is infallible");
    println!("{}", crate::models::ensure_ascii(&pretty));
}

/// click UsageError rendering for a subcommand: usage + Try line + blank +
/// `Error: {msg}` on stderr, exit 2.
fn usage_error(command: &str, message: &str) -> i32 {
    eprintln!("Usage: stado-coverage {command} [OPTIONS] UNIVERSE_ID");
    eprintln!("Try 'stado-coverage {command} --help' for help.");
    eprintln!();
    eprintln!("Error: {message}");
    2
}

/// A runtime failure after argument parsing (Python: uncaught exception
/// traceback, exit 1; here a clean `Error: {msg}` line, same exit code).
fn runtime_error(err: &CoverageError) -> i32 {
    eprintln!("Error: {err}");
    1
}

#[derive(clap::Parser)]
#[command(
    name = "stado-coverage",
    about = "Verify + retry job-completion coverage for a registered universe."
)]
struct Cli {
    #[command(subcommand)]
    command: CoverageCommands,
}

#[derive(clap::Subcommand)]
enum CoverageCommands {
    /// List registered coverage universes.
    List,
    /// Dry-run coverage walk; print per-universe report JSON. No submits.
    Verify {
        universe_id: String,
        /// Universe constructor kwarg KEY=VALUE; repeat per kwarg.
        #[arg(long = "kv")]
        kv_pairs: Vec<String>,
    },
    /// Verify, and with --execute, re-submit MISSING tuples via submit_batch.
    Retry {
        universe_id: String,
        /// Universe constructor kwarg KEY=VALUE; repeat per kwarg.
        #[arg(long = "kv")]
        kv_pairs: Vec<String>,
        /// Actually submit gap jobs via submit_batch; default is dry-run.
        #[arg(long)]
        execute: bool,
    },
}

/// The `stado-coverage` entry point (click group). Exit codes match click:
/// 2 for usage errors (clap parse failures and UsageError equivalents),
/// 1 for runtime failures and the empty-universe `list`, 0 on success.
pub async fn cli_main() -> i32 {
    let cli = <Cli as clap::Parser>::parse();
    let log = |msg: String| eprintln!("{msg}");
    match cli.command {
        CoverageCommands::List => {
            let names = list_universes();
            if names.is_empty() {
                eprintln!("(no universes registered)");
                return 1;
            }
            for name in names {
                println!("{name}");
            }
            0
        }
        CoverageCommands::Verify {
            universe_id,
            kv_pairs,
        } => {
            let universe = match kv_to_kwargs(&kv_pairs)
                .and_then(|kwargs| build_universe(&universe_id, kwargs))
            {
                Ok(universe) => universe,
                Err(message) => return usage_error("verify", &message),
            };
            let result = async {
                let store = JobStorage::with_bucket(config::bucket()).await?;
                let state = state_load(&store, universe.id()).await?;
                verify(
                    universe.as_ref(),
                    config::COVERAGE_VERIFY_THREADS as usize,
                    &state,
                    Some(&log),
                )
                .await
            }
            .await;
            match result {
                Ok(report) => {
                    print_pretty(&report.as_dict());
                    0
                }
                Err(err) => runtime_error(&err),
            }
        }
        CoverageCommands::Retry {
            universe_id,
            kv_pairs,
            execute,
        } => {
            let universe = match kv_to_kwargs(&kv_pairs)
                .and_then(|kwargs| build_universe(&universe_id, kwargs))
            {
                Ok(universe) => universe,
                Err(message) => return usage_error("retry", &message),
            };
            match verify_and_retry(
                universe.as_ref(),
                execute,
                config::COVERAGE_VERIFY_THREADS as usize,
                Some(&log),
            )
            .await
            {
                Ok(report) => {
                    print_pretty(&report.as_dict());
                    0
                }
                Err(err) => runtime_error(&err),
            }
        }
    }
}
