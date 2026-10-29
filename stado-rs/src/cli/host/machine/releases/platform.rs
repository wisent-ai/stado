use crate::cli::entry::spec::fleet::host::runs::HostCargoOperation;
use crate::cli::CmdError;
use crate::deploy::host_run::{CargoOperation, HostCargoReceipt, HostCargoRequest};

use crate::cli::host::checks::probes::print_json;

/// What `stado host cargo` was given on its command line.
pub(crate) struct HostCargoArguments<'a> {
    pub target: &'a str,
    pub operation: HostCargoOperation,
    pub manifest_path: &'a str,
    pub revision: &'a str,
    pub binaries: Vec<String>,
    pub arguments: Vec<String>,
    pub json: bool,
}

/// `stado host cargo TARGET OPERATION --manifest-path PATH --revision REV
/// [--bin NAME]... [--json] [-- ARGS]` — one Cargo operation on a committed
/// revision, run on TARGET in a managed run of its own.
///
/// The variable inputs select a checkout's manifest, a commit, binaries and
/// Cargo arguments; they never select a shell command, the toolchain path,
/// the target directory or the working directory.
pub(crate) async fn cargo(arguments: HostCargoArguments<'_>) -> Result<(), CmdError> {
    let json_output = arguments.json;
    let request = HostCargoRequest {
        manifest_path: std::path::PathBuf::from(arguments.manifest_path),
        revision: arguments.revision.to_string(),
        operation: match arguments.operation {
            HostCargoOperation::Build => CargoOperation::Build,
            HostCargoOperation::Check => CargoOperation::Check,
            HostCargoOperation::Clippy => CargoOperation::Clippy,
            HostCargoOperation::Test => CargoOperation::Test,
        },
        binaries: arguments.binaries,
        arguments: arguments.arguments,
    };
    crate::deploy::host_run::validate_cargo(&request)
        .map_err(|error| CmdError::usage(error.to_string()).machine_readable(json_output))?;
    let resolved = crate::cli::canonical_host(arguments.target)
        .await
        .map_err(|error| error.machine_readable(json_output))?;
    let receipt =
        crate::deploy::host_run::cargo(&resolved, &request, &crate::deploy::production_runner())
            .await
            .map_err(|error| CmdError::from(error).machine_readable(json_output))?;
    let exit_code = receipt.exit_code;
    if json_output {
        print_json(&serde_json::to_value(&receipt)?);
    } else {
        print!("{}", receipt.stdout);
        eprint!("{}", receipt.stderr);
        for artifact in &receipt.artifacts {
            eprintln!(
                "{}: {} ({} bytes, sha256 {})",
                artifact.binary,
                artifact.path.display(),
                artifact.bytes,
                artifact.sha256
            );
        }
        eprintln!("{}", space_line(&receipt));
    }
    if exit_code == nix::libc::EXIT_SUCCESS {
        Ok(())
    } else {
        Err(CmdError::silent(exit_code))
    }
}

/// What the run took from the target's volume, or why that is not known.
fn space_line(receipt: &HostCargoReceipt) -> String {
    let target = &receipt.target;
    let commit = &receipt.commit;
    match (receipt.consumed_gib(), &receipt.free_bytes_after_error) {
        (Some(gib), _) => format!(
            "{target}: cargo {} of {commit} took {gib:.1} GiB of free space on its volume \
             (`stado space report {target}`)",
            receipt.operation.word()
        ),
        (None, Some(error)) => {
            format!("{target}: the volume's free space after the run was not read: {error}")
        }
        (None, None) => format!(
            "{target}: the host reported no free-space reading, so the space the run took is \
             not known"
        ),
    }
}

/// `stado host run attach TARGET --program PATH [--arg ARG]...` — attach
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
        .map_err(|error| CmdError::from(error).machine_readable(json_output))?;
    if json_output {
        print_json(&serde_json::to_value(&outcome)?);
    }
    match outcome.exit_code {
        Some(code) if code == nix::libc::EXIT_SUCCESS => Ok(()),
        Some(code) => Err(CmdError::silent(code)),
        None => Err(CmdError::click(format!(
            "{program} on {target} ended with {} and returned no exit code",
            outcome.ended
        ))
        .machine_readable(json_output)),
    }
}
