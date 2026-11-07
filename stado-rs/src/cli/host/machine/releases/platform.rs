use crate::cli::entry::spec::fleet::host::runs::HostCargoOperation;
use crate::cli::CmdError;
use crate::deploy::host_run::{CargoOperation, Ending, HostCargoReceipt, HostCargoRequest};

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
/// inherits stdout and relays stderr chunk by chunk as it arrives, partial
/// lines included; stderr passes through Stado because the host's wrapper
/// reports there the program's own status the moment it ends and the stages
/// after it (the read of which processes still hold its output, the channel
/// closing), each a wait Stado opens and settles as the lines pass. The JSON
/// form captures both streams only because a single machine-readable receipt
/// cannot share stdout with an arbitrary program protocol; its receipt keeps
/// the program's status as `program_exit` beside the channel's `exit_code`.
///
/// The command exits with the program's own status. When the host's wrapper
/// ended the run before the program reported one, it exits with the
/// wrapper's status, whose words are on stderr; when the channel ended
/// first, it fails saying so, since whether the program ran is not known.
pub async fn run_attached(
    target: &str,
    program: &str,
    arguments: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    if let Err(error) = crate::deploy::host_run::validate_run_descendant(program) {
        return Err(CmdError::usage(error)
            .helping(format!(
                "attach starts only a program delivered into a run of its own, so the bytes it \
                 starts are the ones that run's delivery placed and a host's installed program is \
                 never driven through it. Deliver the program first with `stado host run deliver \
                 {target} <file> .stado/work/runs/<id>/<name>` and attach that path; `stado host \
                 upgrade {target}` brings {target}'s installed Stado to a newer revision, \
                 delivering, validating and installing it this way; `stado host exec {target}` \
                 runs the installed Stado's approved reads."
            ))
            .machine_readable(json_output));
    }
    crate::deploy::host_run::validate_arguments(arguments)
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
    match outcome.judged() {
        Ok(()) => Ok(()),
        Err(failure) => match (failure.ending, outcome.program_exit, outcome.exit_code) {
            (Ending::Program, Some(code), _) | (Ending::Host, None, Some(code)) => {
                Err(CmdError::silent(code))
            }
            _ => Err(
                CmdError::unreachable(format!("{program} on {target} {}", failure.said))
                    .machine_readable(json_output),
            ),
        },
    }
}
