//! Which `stado serve` roles one live process runs, answered on its own host.
//!
//! The answer comes from the kernel's argument vector, parsed by the same
//! [`ServeArgs`] definition `serve` itself reads. A rendered `ps` command line
//! cannot tell an option from words inside another option's value, so a
//! pattern argument such as `'x --resolver'` would read as a resolver role;
//! parsing the real vector cannot make that mistake.
//!
//! The vector being read was written by whichever build started the process
//! or rendered the unit, which is usually an older one. An option a later
//! build removed is still on those hosts, so it is carried as opaque text
//! beside the line rather than refused: refusing it made every unit holding a
//! removed option unreadable to every newer build, and no upgrade could then
//! retire it. Only the options this build declares decide roles.

use clap::{CommandFactory, Parser};

use crate::cli::CmdError;

use super::ServeArgs;

/// `serve`'s own argument definition, fed the vector after the program name.
#[derive(Parser)]
struct ServeLine {
    #[command(flatten)]
    args: ServeArgs,
}

/// A serve vector as this build reads it: the line its own options make, and
/// every option it does not declare, kept verbatim (with the value that
/// follows it) because an earlier build wrote it.
struct ReadServeLine {
    line: ServeLine,
    opaque: Vec<String>,
}

/// Read `words` (`serve` first) as this build's serve line. Options this
/// build does not declare are set aside as opaque text: `serve` takes no
/// positional argument, so a word after such an option that is not itself an
/// option can only be that option's value and goes with it. What remains is
/// parsed by serve's own definition, and a vector that still does not parse
/// (a declared option with a value it refuses, a missing required option) is
/// clap's own reason, never read as running no role.
fn read_serve_line(words: &[&str]) -> Result<ReadServeLine, String> {
    let command = ServeLine::command();
    let declared = |name: &str| {
        command.get_arguments().find(|argument| {
            argument.get_long() == Some(name)
                || argument
                    .get_all_aliases()
                    .is_some_and(|aliases| aliases.contains(&name))
        })
    };
    let mut kept: Vec<&str> = Vec::with_capacity(words.len());
    let mut opaque = Vec::new();
    let mut rest = words.iter().copied();
    if let Some(first) = rest.next() {
        kept.push(first);
    }
    let mut rest = rest.peekable();
    while let Some(word) = rest.next() {
        let Some(option) = word.strip_prefix("--").filter(|option| !option.is_empty()) else {
            kept.push(word);
            continue;
        };
        let (name, inline_value) = match option.split_once('=') {
            Some((name, _)) => (name, true),
            None => (option, false),
        };
        match declared(name) {
            Some(argument) => {
                kept.push(word);
                if !inline_value && argument.get_action().takes_values() {
                    if let Some(value) = rest.next() {
                        kept.push(value);
                    }
                }
            }
            None => {
                let mut text = word.to_string();
                if !inline_value {
                    if let Some(value) = rest.next_if(|next| !next.starts_with('-')) {
                        text.push(' ');
                        text.push_str(value);
                    }
                }
                opaque.push(text);
            }
        }
    }
    let line = ServeLine::try_parse_from(&kept).map_err(|error| {
        format!(
            "`{}` cannot be read by this build's serve: {error}",
            words.join(" ")
        )
    })?;
    Ok(ReadServeLine { line, opaque })
}

/// Print `STADO_SERVE_ROLES` and the role options process `pid` was started
/// with, then `STADO_SERVE_ROLE_PATHS` with the program, root or destination
/// each path-carrying role names, one `--option=value` per field. A process
/// that is not `stado serve` has no roles and prints both markers with
/// nothing after them; a vector `serve` would refuse is an error, never a
/// guess. Options an earlier build wrote and this one does not declare are
/// named on stderr, read as no role.
pub(crate) fn print_roles(pid: u32) -> Result<(), CmdError> {
    // A vector that cannot be read belongs to a process that is gone, which
    // is not_found, or to one the kernel would not describe, which is the
    // host's failure; the process table, not the sentence, tells them apart.
    let argv = crate::deploy::service::process_arguments(pid).map_err(|detail| {
        let code = if crate::providers::local::helpers::pid_alive(pid as i32) {
            crate::primitives::failure::FailureCode::InfraDown
        } else {
            crate::primitives::failure::FailureCode::NotFound
        };
        CmdError::click(detail).stating(code)
    })?;
    let words: Vec<&str> = argv.iter().map(String::as_str).collect();
    let (roles, paths) = if let [_program, serve @ ..] = words.as_slice() {
        if serve.first() == Some(&"serve") {
            let read = read_serve_line(serve).map_err(|error| {
                CmdError::click(format!("PID {pid} runs a serve line that {error}"))
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
            if !read.opaque.is_empty() {
                eprintln!(
                    "stado: PID {pid} carries options this build does not declare, read as no role: {}",
                    read.opaque.join(" ")
                );
            }
            (roles(&read.line.args), role_paths(&read.line.args))
        } else {
            (Vec::new(), Vec::new())
        }
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
/// each one-pass command a native timer can repeat maps to the role that
/// repeats it inside the one process. This is Stado's knowledge of its own
/// commands, and it is how a unit running one of them is matched to the role
/// that replaced it without any list of unit names. Empty for a line that is
/// no Stado role, including a command this build no longer has. A serve line
/// is read by [`read_serve_line`]: an option this build no longer declares is
/// read as no role, and a line that still does not parse is clap's reason,
/// never read as running no role.
pub(crate) fn command_roles(words: &[&str]) -> Result<Vec<&'static str>, String> {
    Ok(match words {
        ["serve", ..] => roles(&read_serve_line(words)?.line.args),
        ["product", rest @ ..] if product_verb(rest) == Some("sync") => {
            vec!["--product-sync-interval-seconds"]
        }
        ["host", "beacon", "publish" | "collect", ..] => vec!["--health-interval-seconds"],
        // Units installed by builds before `host beacon` still run these
        // words; they are read here because they are on hosts, not because
        // this build accepts them.
        ["host", "publish-beacon" | "collect-beacon", ..] => vec!["--health-interval-seconds"],
        _ => Vec::new(),
    })
}

/// The role lock a `stado serve` line takes on its host
/// ([`crate::deploy::role_lock::serve_role`]): `serve`, or `serve--<target>`
/// for a line serving one worker target. Two lines with the same lock are
/// the same process's, never two beside each other. `None` for a line that
/// is not `serve`; a serve line is read by [`read_serve_line`].
pub(crate) fn serve_lock(words: &[&str]) -> Result<Option<String>, String> {
    if words.first() != Some(&"serve") {
        return Ok(None);
    }
    let line = read_serve_line(words)?.line;
    let target = line.args.worker.target.as_deref();
    Ok(Some(crate::deploy::role_lock::serve_role(target)))
}

/// The group's own options that take a value: `--catalog PATH`, another
/// authority file, and `--workspace DIR`, another workspace of checkouts.
const PRODUCT_VALUE_OPTIONS: &[&str] = &["--catalog", "--workspace"];

/// The verb of a `stado product` line, past the group's own value options
/// (`--catalog PATH`, `--workspace=DIR`), which a unit names before the verb.
fn product_verb<'a>(words: &[&'a str]) -> Option<&'a str> {
    let mut words = words.iter().copied();
    while let Some(word) = words.next() {
        if PRODUCT_VALUE_OPTIONS.contains(&word) {
            words.next();
        } else if !PRODUCT_VALUE_OPTIONS
            .iter()
            .any(|option| word.starts_with(&format!("{option}=")))
        {
            return Some(word);
        }
    }
    None
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
        (
            args.failure_fixer_interval_seconds.is_some(),
            "--failure-fixer-interval-seconds",
        ),
        (args.watchdog, "--watchdog"),
        (args.forward_destination.is_some(), "--forward-destination"),
        (args.edge_caddyfile.is_some(), "--edge-caddyfile"),
        (args.precheck_runner.is_some(), "--precheck-runner"),
        (args.release_loop_root.is_some(), "--release-loop-root"),
        (args.public_status_bind.is_some(), "--public-status-bind"),
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
    if let Some(root) = &args.release_loop_root {
        paths.push(("--release-loop-root", root.display().to_string()));
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
