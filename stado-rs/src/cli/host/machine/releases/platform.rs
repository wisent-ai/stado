use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;

/// `stado host build TARGET --manifest-path PATH --bin NAME [--json]` —
/// execute the one Cargo build Stado declares for a delivered source tree.
///
/// The variable inputs select a manifest and one binary; they never select a
/// shell command, Cargo flags, toolchain path, or working directory. Both the
/// lexical preflight and the host-side physical-path check bind the manifest
/// below the approved account's `$HOME/.stado/work/runs`.
pub async fn build(
    target: &str,
    manifest_path: &str,
    binary: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    crate::deploy::host_run::validate_run_descendant(manifest_path)
        .and_then(|_| crate::deploy::host_run::validate_binary_name(binary))
        .map_err(|error| CmdError::usage(error).machine_readable(json_output))?;
    let resolved = crate::cli::canonical_host(target)
        .await
        .map_err(|error| error.machine_readable(json_output))?;
    let outcome = crate::deploy::host_run::build(
        &resolved,
        manifest_path,
        binary,
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    let exit_code = outcome.exit_code;
    if json_output {
        print_json(&serde_json::to_value(&outcome)?);
    } else {
        print!("{}", outcome.stdout);
        eprint!("{}", outcome.stderr);
    }
    if exit_code == 0 {
        Ok(())
    } else {
        Err(CmdError::silent(exit_code))
    }
}

/// `stado host run-attached TARGET --program PATH [--arg ARG]...` — attach
/// this process to one executable below the target account's managed run tree.
///
/// Standard input is inherited rather than read into a string, so a credential
/// body does not enter Stado's arguments, logs, or receipts. The non-JSON form
/// also inherits both output streams byte-for-byte. The JSON form captures
/// them only because a single machine-readable receipt cannot share stdout
/// with an arbitrary program protocol.
pub async fn run_attached(
    target: &str,
    program: &str,
    arguments: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    crate::deploy::host_run::validate_run_descendant(program)
        .and_then(|_| crate::deploy::host_run::validate_arguments(arguments))
        .map_err(|error| CmdError::usage(error).machine_readable(json_output))?;
    let resolved = crate::cli::canonical_host(target)
        .await
        .map_err(|error| error.machine_readable(json_output))?;
    let outcome = crate::deploy::host_run::run_attached(&resolved, program, arguments, json_output)
        .await
        .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    let exit_code = outcome.exit_code;
    if json_output {
        print_json(&serde_json::to_value(&outcome)?);
    }
    if exit_code == 0 {
        Ok(())
    } else {
        Err(CmdError::silent(exit_code))
    }
}
