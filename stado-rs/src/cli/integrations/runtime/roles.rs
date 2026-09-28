//! Which `stado serve` roles one live process runs, answered on its own host.
//!
//! The answer comes from the kernel's argument vector, parsed by the same
//! [`ServeArgs`] definition `serve` itself reads. A rendered `ps` command line
//! cannot tell an option from words inside another option's value, so a
//! pattern argument such as `'x --resolver'` would read as a resolver role;
//! parsing the real vector cannot make that mistake.

use clap::Parser;

use crate::cli::CmdError;

use super::ServeArgs;

/// `serve`'s own argument definition, fed the vector after the program name.
#[derive(Parser)]
struct ServeLine {
    #[command(flatten)]
    args: ServeArgs,
}

/// Print `STADO_SERVE_ROLES` and the role options process `pid` was started
/// with. A process that is not `stado serve` has no roles and prints the
/// marker with nothing after it; a vector `serve` would refuse is an error,
/// never a guess.
pub(crate) fn print_roles(pid: u32) -> Result<(), CmdError> {
    let argv = crate::deploy::service::process_arguments(pid).map_err(CmdError::click)?;
    let roles = if argv.get(1).map(String::as_str) == Some("serve") {
        let line = ServeLine::try_parse_from(&argv[1..]).map_err(|error| {
            CmdError::click(format!(
                "PID {pid} runs a serve line this build cannot read: {error}"
            ))
        })?;
        roles(&line.args)
    } else {
        Vec::new()
    };
    println!("STADO_SERVE_ROLES\t{}", roles.join(" "));
    Ok(())
}

/// Print `STADO_RESOLVER_STATE` with what the resolver last published on this
/// host: the proof a resolver role serves, read whether or not a replacement
/// process runs, because a handoff has to be judged after that process died.
/// Nothing when no resolver has published.
pub(crate) fn print_resolver_state() {
    if let Some(line) = crate::cli::resolver::readiness_marker() {
        println!("{line}");
    }
}

/// The option that switches on each role `args` runs, in the words the
/// catalog's `role_units` name them.
fn roles(args: &ServeArgs) -> Vec<&'static str> {
    [
        (args.resolver, "--resolver"),
        (
            args.release_interval_seconds.is_some(),
            "--release-interval-seconds",
        ),
        (args.control_plane.is_some(), "--control-plane"),
        (
            args.health_interval_seconds.is_some(),
            "--health-interval-seconds",
        ),
        (args.run_worker, "--worker"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect()
}
