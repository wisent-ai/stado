//! The process entry point, and the match that lands a parsed command on its
//! implementation.
//!
//! [`main_entry`] parses argv, runs the dispatched command and presents
//! whatever went wrong; `failure` builds the two keys a failure is logged
//! under; `routes` holds the match arms, grouped exactly as
//! [`super::spec::root`] groups the variants they answer.

use clap::{CommandFactory, FromArgMatches};

use super::spec::{Cli, Commands};
use crate::cli::onboarding;
use crate::cli::{CmdError, CLICK_ERROR_CODE};

mod failure;
mod routes;

use failure::{failure_point, failure_service};

/// Parse argv and run the dispatched command, then present whatever went
/// wrong to the operator.
///
/// A failure leaves three things behind, in this order: the command's own
/// `Error: {msg}` line, unchanged and unabridged; one classified sentence
/// saying whether this is our outage or their request and whether a retry
/// can help; and one structured log line for whatever ships this host's
/// stderr. There is no fourth thing — in particular no HTTP call to an
/// analytics collector, which on a failure path is just one more dependency
/// that can hang the tool.
///
/// Exit codes:
/// 0 on success, [`CLICK_ERROR_CODE`] on a runtime error, 2 on
/// usage errors (clap parse failures exit 2 on their own) and for
/// not-yet-implemented commands, and [`crate::failure::retry_exit_code`]
/// when the failure is one a retry can clear. See `stado.wisent.com/docs/cli`.
pub async fn main_entry() -> i32 {
    // Parse in two steps rather than through `Cli::parse()` — which is
    // exactly these two steps — so the matches tree is still in hand
    // afterwards. It is the only place the subcommand path exists as data
    // rather than as a match arm, and that path is the failure point.
    let matches = Cli::command().get_matches();
    let point = failure_point(&matches);
    let service = failure_service(&matches);
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(err) => err.exit(),
    };
    match dispatch(cli).await {
        // Success.
        Ok(()) => i32::default(),
        Err(err) => {
            // A command that printed its own diagnosis and then chose to say
            // nothing more has already answered; adding a classification
            // line here would contradict it.
            let Some(message) = err.message.as_deref() else {
                return err.code;
            };
            // What the failure said about itself beats what its wording
            // looks like. `classify_message` reads prose, and prose is
            // evidence only when there is nothing better: it once read a
            // refusal's own allowlist and reported `timeout, retryable`.
            let code = err
                .failure
                .unwrap_or_else(|| crate::failure::classify_message(message));
            if err.json {
                // The same sorted-keys rendering every `--json` command on
                // this CLI already prints, so a caller parses one shape.
                println!(
                    "{}",
                    crate::deploy::host_recovery::to_sorted_pretty(&serde_json::json!({
                        "status": "error",
                        "failure_point": point,
                        "service": service,
                        "error_code": code.as_str(),
                        "retryable": code.retryable(),
                        "severity": code.severity().as_str(),
                        "summary": code.operator_summary(),
                        "message": message,
                        "help": err.help,
                    }))
                );
            } else {
                eprintln!("Error: {message}");
                if let Some(help) = err.help.as_deref() {
                    eprintln!("{help}");
                }
                eprintln!("{}", crate::failure::operator_line(code));
            }
            crate::failure::log_failure(&point, service, code, message);
            // Usage errors keep their own code: no amount of retrying fixes
            // an argument, whatever the message happens to read like.
            if err.code == CLICK_ERROR_CODE {
                code.exit_code(err.code)
            } else {
                err.code
            }
        }
    }
}

async fn dispatch(cli: Cli) -> Result<(), CmdError> {
    let catalog_problems = crate::capabilities::validate_catalog();
    if !catalog_problems.is_empty() {
        return Err(CmdError::click(format!(
            "capability catalog is invalid: {}",
            catalog_problems.join("; ")
        )));
    }
    let Some(command) = cli.command else {
        return onboarding::run(false, None, false).await;
    };
    match command {
        Commands::Installation(command) => routes::installation::dispatch(command).await,
        Commands::Work(command) => routes::work::dispatch(command).await,
        Commands::Planes(command) => routes::planes::dispatch(command).await,
        Commands::Platform(command) => routes::platform::dispatch(command).await,
    }
}
