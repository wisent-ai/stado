//! `stado product`: the canonical Wisent product catalog and every product
//! lifecycle operation — catalog, registry, creation, installation, signing,
//! scheduled updates, and canonical Cargo and Swift source builds.
//!
//! The catalog is `catalog/products.yml` at the root of this repository and
//! the implementation is the `stado-product` crate beside this one. Both were
//! the separate `wisent-products` program until Stado 0.22.0; Stado already owns
//! the hosts, services and releases those operations act on, so they run here
//! in the same process rather than through a second executable Stado had to
//! download before it could install anything.

use clap::{parser::ValueSource, ArgMatches, Command, FromArgMatches, Subcommand};

use crate::cli::CmdError;

pub use stado_product::SIGNING_ROLE;

/// One parsed `stado product` invocation. Its operations are declared once, by
/// the product crate, and parsed by the same clap tree as the rest of Stado.
#[derive(Debug, Clone)]
pub struct ProductCommands {
    matches: ArgMatches,
}

impl FromArgMatches for ProductCommands {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            matches: matches.clone(),
        })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        self.matches = matches.clone();
        Ok(())
    }
}

impl Subcommand for ProductCommands {
    fn augment_subcommands(command: Command) -> Command {
        stado_product::cli::augment(command)
    }

    fn augment_subcommands_for_update(command: Command) -> Command {
        stado_product::cli::augment(command)
    }

    fn has_subcommand(name: &str) -> bool {
        stado_product::cli::augment(Command::new("product"))
            .find_subcommand(name)
            .is_some()
    }
}

/// The build every product record names.
pub fn build() -> stado_product::Build {
    stado_product::Build {
        version: env!("CARGO_PKG_VERSION"),
        source_revision: env!("STADO_SOURCE_REVISION"),
        signing_role: SIGNING_ROLE,
        product: env!("CARGO_PKG_NAME"),
    }
}

/// The host and the `stado` arguments of a service-surface operation on
/// another registry host, or `None` when it runs here.
///
/// A service installation builds and places files on the machine that runs
/// it and then ensures the unit on `--host`, and its lifecycle record is kept
/// on that machine too; run here for another host it would restart that
/// host's unit on the files it already has and record state for a host that
/// never received it. That host's own Stado runs it instead, through the fleet
/// channel `declare-publisher` and fleet databases already use, so the build,
/// the placed files, the restarted unit and the record are on the host the
/// operator named.
async fn remote_service_operation(
    matches: &ArgMatches,
) -> Result<Option<(String, Vec<String>)>, CmdError> {
    let Some((action, arguments)) = matches.subcommand() else {
        return Ok(None);
    };
    let surface = given(arguments, "surface")?;
    let host = given(arguments, "host")?;
    let (Some(surface), Some(host)) = (surface, host) else {
        return Ok(None);
    };
    if surface != "service" {
        return Ok(None);
    }
    let target = crate::cli::canonical_host(host).await?;
    if crate::deploy::host_channel::target_is_this_host(&target) {
        return Ok(None);
    }
    if matches.get_one::<String>("catalog").is_some() {
        return Err(CmdError::refused(format!(
            "--catalog names a file on this machine, and a service {action} for {name} runs on \
             {name} with that host's own catalog. Nothing was built, installed or restarted",
            name = target.name
        )));
    }
    let mut words = vec!["product".to_string(), action.to_string()];
    words.extend(command_line_words(action, arguments, &target.name));
    // `--purpose` is global, so the verb's own definition does not list it:
    // carried here, the host's Stado names the same defects to its Oko.
    for purpose in purposes(matches) {
        words.extend(["--purpose".to_string(), purpose.to_string()]);
    }
    Ok(Some((target.name.clone(), words)))
}

/// The open defects the operator named with `--purpose`, in the order given.
fn purposes(matches: &ArgMatches) -> Vec<&str> {
    matches
        .get_many::<String>("purpose")
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect()
}

/// The text `id` was given in `arguments`; `None` when it was not given or
/// this verb declares no such argument. A value of another type is the
/// product crate's definition disagreeing with this reader, said as such.
fn given<'a>(arguments: &'a ArgMatches, id: &str) -> Result<Option<&'a String>, CmdError> {
    match arguments.try_get_one::<String>(id) {
        Ok(value) => Ok(value),
        Err(clap::parser::MatchesError::UnknownArgument { .. }) => Ok(None),
        Err(error) => Err(CmdError::click(format!(
            "the product command's argument --{id} cannot be read as text: {error}"
        ))),
    }
}

/// The arguments the operator gave `action`, rebuilt from its own definition:
/// positionals first, then every option and flag given on the command line,
/// with `--host` naming the host canonically so its own Stado recognises
/// itself. Defaults are left to the host.
fn command_line_words(action: &str, arguments: &ArgMatches, host: &str) -> Vec<String> {
    let product = stado_product::cli::augment(Command::new("product"));
    let Some(definition) = product.find_subcommand(action) else {
        return Vec::new();
    };
    let mut positionals = Vec::new();
    let mut options = Vec::new();
    for argument in definition.get_arguments() {
        let id = argument.get_id().as_str();
        if arguments.value_source(id) != Some(ValueSource::CommandLine) {
            continue;
        }
        let values = arguments
            .get_raw(id)
            .into_iter()
            .flatten()
            .map(|value| value.to_string_lossy().into_owned());
        let Some(long) = argument.get_long() else {
            positionals.extend(values);
            continue;
        };
        if !argument.get_action().takes_values() {
            options.push(format!("--{long}"));
        } else if id == "host" {
            options.extend([format!("--{long}"), host.to_string()]);
        } else {
            for value in values {
                options.extend([format!("--{long}"), value]);
            }
        }
    }
    positionals.extend(options);
    positionals
}

mod janitor;
use janitor::janitor_hold;

pub async fn dispatch(command: ProductCommands) -> Result<(), CmdError> {
    if let Some((host, words)) = remote_service_operation(&command.matches).await? {
        let arguments: Vec<&str> = words.iter().map(String::as_str).collect();
        let output = crate::cli::host::remote_stado_build_output(&host, &arguments).await?;
        print!("{output}");
        return Ok(());
    }
    // Taken first and released when this function returns, after the
    // operation has answered: a build the janitor's cleanup holds is refused
    // for that reason before Oko is asked anything about it.
    let _hold = janitor_hold(&command.matches).await?;
    // An operation that compiles runs only while the operator's rulings allow
    // building this product, as Oko answers it; nothing here keeps the rule.
    if stado_product::cli::compiles(&command.matches) {
        let product = match command.matches.subcommand() {
            Some((_, arguments)) => given(arguments, "positional")?.map(String::as_str),
            None => None,
        };
        crate::cli::release_loop::allowed::require(
            crate::cli::release_loop::allowed::BUILD,
            product,
            &purposes(&command.matches),
        )
        .await?;
    }
    // Product operations run compilers, codesign and `stado` subcommands and
    // wait for them: blocking work, run in place on the thread the CLI runs
    // on, whose stack the binary sizes for its deepest command. An install
    // walks the dependency graph by recursion, one `perform` frame per
    // product carrying the whole plan, and overflowed a runtime worker's
    // stack on the second level of a debug build.
    let status = tokio::task::block_in_place(|| stado_product::cli::run(command.matches, build()))
        .map_err(|error| {
            let message = match product_refusal(&error) {
                Some(cause) => format!("{error:#}; {cause}"),
                None => format!("{error:#}"),
            };
            CmdError::click(message).stating(product_failure_code(&error))
        })?;
    if status == nix::libc::EXIT_SUCCESS {
        Ok(())
    } else {
        Err(CmdError::silent(status))
    }
}

/// Product failures carry their classification in the cause chain. A retained
/// installation bound to another host, and a source commit no canonical
/// branch carries, are the request refusing itself; a build of a committed
/// manifest its own Cargo.lock does not cover, a repository with no canonical
/// checkout, and a catalog an installation was given that its checkout has
/// not committed, are the declared state refusing, which no retry
/// changes. A child (`stado credentials get` reading the signing credential
/// from a Skarbiec that does not answer) that exited with the fleet's retry
/// exit code said that what it needs is unreachable now. Otherwise the first
/// IO or JSON cause decides; wording is never classified.
fn product_failure_code(error: &anyhow::Error) -> crate::primitives::failure::FailureCode {
    // An update policy that cannot be followed is attached as context, which
    // anyhow finds through the error itself and never as a link of its chain.
    if error
        .downcast_ref::<stado_product::InvalidPolicy>()
        .is_some()
    {
        return crate::primitives::failure::FailureCode::Config;
    }
    for cause in error.chain() {
        if cause
            .downcast_ref::<stado_product::cargo_lock::UnlockedManifest>()
            .is_some()
            || cause
                .downcast_ref::<stado_product::MissingCheckout>()
                .is_some()
            || cause
                .downcast_ref::<stado_product::catalog::committed::Uncommitted>()
                .is_some()
        {
            return crate::primitives::failure::FailureCode::Config;
        }
        if cause
            .downcast_ref::<stado_product::InstallationHostConflict>()
            .is_some()
            || cause
                .downcast_ref::<stado_product::NotCanonical>()
                .is_some()
        {
            return crate::primitives::failure::FailureCode::Refused;
        }
        if cause
            .downcast_ref::<stado_product::common::CommandFailed>()
            .is_some_and(|failed| {
                failed.code == Some(crate::primitives::failure::FailureCode::RETRY_EXIT)
            })
        {
            return crate::primitives::failure::FailureCode::InfraDown;
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return crate::cli::entry::error::io_failure_code(io.kind());
        }
        if let Some(json) = cause.downcast_ref::<serde_json::Error>() {
            if !json.is_io() {
                return crate::primitives::failure::FailureCode::Refused;
            }
        }
    }
    crate::primitives::failure::FailureCode::Unknown
}

/// The operating system's reason for the first refused file in the chain,
/// when the kernel's answer alone does not say who has to allow what.
fn product_refusal(error: &anyhow::Error) -> Option<String> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .and_then(crate::cli::entry::error::operating_system_refusal)
}
