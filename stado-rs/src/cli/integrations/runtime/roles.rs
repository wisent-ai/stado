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
/// with, then `STADO_SERVE_ROLE_PATHS` with the program, root or destination
/// each path-carrying role names, one `--option=value` per field. A process
/// that is not `stado serve` has no roles and prints both markers with
/// nothing after them; a vector `serve` would refuse is an error, never a
/// guess.
pub(crate) fn print_roles(pid: u32) -> Result<(), CmdError> {
    let argv = crate::deploy::service::process_arguments(pid).map_err(CmdError::click)?;
    let (roles, paths) = if argv.get(1).map(String::as_str) == Some("serve") {
        let line = ServeLine::try_parse_from(&argv[1..]).map_err(|error| {
            CmdError::click(format!(
                "PID {pid} runs a serve line this build cannot read: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        (roles(&line.args), role_paths(&line.args))
    } else {
        (Vec::new(), Vec::new())
    };
    println!("STADO_SERVE_ROLES\t{}", roles.join(" "));
    let paths: Vec<String> = paths
        .into_iter()
        .map(|(flag, value)| format!("{flag}={value}"))
        .collect();
    println!("STADO_SERVE_ROLE_PATHS\t{}", paths.join("\t"));
    Ok(())
}

/// The `stado serve` roles a Stado command line does the work of, read from
/// the words after the program. `serve` is parsed by serve's own definition;
/// each standalone command the one process folded in maps to the role that
/// now runs its work. This is Stado's knowledge of its own commands, and it
/// is how a unit running one of them is matched to the role that replaced it
/// without any list of unit names. Empty for a line that is no Stado role,
/// including a serve line this build cannot read.
pub(crate) fn command_roles(words: &[&str]) -> Vec<&'static str> {
    let Some((&first, rest)) = words.split_first() else {
        return Vec::new();
    };
    match (first, rest.first().copied()) {
        ("serve", _) => ServeLine::try_parse_from(words)
            .map(|line| roles(&line.args))
            .unwrap_or_default(),
        ("dashboard", _) => vec!["--api"],
        ("agent", _) => vec!["--worker"],
        ("coordinator", _) => vec!["--control-plane"],
        ("disk-cleanup", _) => vec!["--disk-cleanup"],
        ("resolver", Some("serve")) => vec!["--resolver"],
        ("release", Some("agent")) => vec!["--release-interval-seconds"],
        ("product", Some("sync")) => vec!["--product-sync-interval-seconds"],
        ("host", Some("publish-beacon" | "collect-beacon")) => vec!["--health-interval-seconds"],
        _ => Vec::new(),
    }
}

/// The roles of the one process that do the work of Stado's own programs
/// other than `stado`, built from the same source: a unit that runs one of
/// them is matched to that role. `None` for any other program.
pub(crate) fn companion_roles(executable: &str) -> Option<Vec<&'static str>> {
    match executable {
        "stado-watchdog" => Some(vec!["--watchdog"]),
        "stado-fix" => Some(vec!["--failure-fixer-interval-seconds"]),
        _ => None,
    }
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

/// The option that switches on each role `args` runs.
fn roles(args: &ServeArgs) -> Vec<&'static str> {
    [
        (args.api, "--api"),
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
        (
            args.product_sync_interval_seconds.is_some(),
            "--product-sync-interval-seconds",
        ),
        (args.run_worker, "--worker"),
        (args.disk_cleanup, "--disk-cleanup"),
        (
            args.failure_fixer_interval_seconds.is_some(),
            "--failure-fixer-interval-seconds",
        ),
        (args.watchdog, "--watchdog"),
        (args.forward_destination.is_some(), "--forward-destination"),
        (args.edge_caddyfile.is_some(), "--edge-caddyfile"),
        (args.precheck_runner.is_some(), "--precheck-runner"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect()
}

/// The program, root or destination each path-carrying role of `args` names,
/// under the role option `roles` reports for it: a unit that runs that
/// program, runs from that root, or forwards to that destination does that
/// role's work.
fn role_paths(args: &ServeArgs) -> Vec<(&'static str, String)> {
    let mut paths = Vec::new();
    if let Some(root) = &args.precheck_runner {
        paths.push(("--precheck-runner", root.display().to_string()));
    }
    for program in [&args.edge_caddy, &args.edge_caddyfile]
        .into_iter()
        .flatten()
    {
        paths.push(("--edge-caddyfile", program.display().to_string()));
    }
    if let Some(destination) = &args.forward_destination {
        paths.push(("--forward-destination", destination.clone()));
    }
    paths
}
