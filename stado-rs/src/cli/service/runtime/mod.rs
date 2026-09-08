//! The runtime surface of a declared unit: the environment it runs with, the
//! files and secrets delivered into it, and what is actually answering on the
//! port it declares.
//!
//! [`print_env_file_head`] and [`env_file_failure`] live here because two
//! commands read the same env file: `env-show` prints it and `endpoint-check`
//! derives the endpoints it checks from it.

use super::*;

pub(crate) mod env;
pub(crate) mod files;
pub(crate) mod secrets;
pub(crate) mod serving;

/// The head every env-file reader prints: which file, how protected, how big.
///
/// The mode is not decoration. This file is the one an operator is about to
/// believe, and a `worker.env` the group can read is a finding that belongs
/// next to its contents rather than in a separate audit nobody runs.
fn print_env_file_head(report: &service_env_file::EnvFileReport) {
    println!(
        "env file: {} ({})",
        dash(&report.path),
        if report.file_state == service_env_file::FILE_READ {
            format!(
                "mode {}, {}, {} bytes",
                dash(&report.mode),
                if report.owner_only {
                    "owner-only"
                } else {
                    "READABLE BEYOND ITS OWNER"
                },
                report.bytes
            )
        } else {
            format!("{}: {}", report.file_state, dash(&report.detail))
        }
    );
}

/// Why a report cannot be believed, or `None` when it can.
///
/// A file that was never opened and a file that holds no assignments are
/// opposite answers, and this command must not exit zero on the first while
/// printing the empty table of the second.
fn env_file_failure(host: &str, report: &service_env_file::EnvFileReport) -> Option<String> {
    if report.file_state != service_env_file::FILE_READ {
        return Some(format!(
            "{host}: {} — {}",
            report.file_state,
            dash(&report.detail)
        ));
    }
    if report.entries_state != service_env_file::ENTRIES_READ {
        return Some(format!(
            "{host}: the file was readable and its assignments were not parsed ({})",
            report.entries_state
        ));
    }
    None
}
