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

/// The role whose item holds the fleet's Apple certificate and key: the vault
/// item carrying `stado:role:macos-development-signing`. It signs every product
/// installation unless the operator names another role through
/// `WISENT_CODESIGN_ROLE` or supplies `WISENT_CODESIGN_CERTIFICATE_PEM`.
///
/// A role says what the secret is for, never which item holds it, so renaming
/// or replacing the item changes nothing here; it is the same role the
/// release manifests name in `secret_env`.
pub const SIGNING_ROLE: &str = "macos-development-signing";

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
    }
}

/// The stack a product operation runs on. An installation walks the
/// product's dependency graph by recursion, one `perform` frame per
/// product, and those frames carry the whole plan; on the 2 MiB a tokio
/// blocking thread offers, a debug build overflowed on the second level.
const PRODUCT_OPERATION_STACK: usize = 256 << 20;

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
    let surface = arguments.try_get_one::<String>("surface").ok().flatten();
    let host = arguments.try_get_one::<String>("host").ok().flatten();
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
    Ok(Some((target.name.clone(), words)))
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

pub async fn dispatch(command: ProductCommands) -> Result<(), CmdError> {
    if let Some((host, words)) = remote_service_operation(&command.matches).await? {
        let arguments: Vec<&str> = words.iter().map(String::as_str).collect();
        let output = crate::cli::host::remote_stado_build_output(&host, &arguments).await?;
        print!("{output}");
        return Ok(());
    }
    // Product operations run compilers, codesign and `stado` subcommands and
    // wait for them; they are blocking work, kept off the async workers, on a
    // thread of their own with the stack the dependency walk needs.
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("product-operation".into())
        .stack_size(PRODUCT_OPERATION_STACK)
        .spawn(move || {
            let _ = sender.send(stado_product::cli::run(command.matches, build()));
        })
        .map_err(|error| CmdError::click(format!("product operation could not start: {error}")))?;
    let status = receiver
        .await
        .map_err(|_| CmdError::click("product operation stopped without an answer"))?
        .map_err(|error| {
            CmdError::click(format!("{error:#}")).stating(product_failure_code(&error))
        })?;
    if status == 0 {
        Ok(())
    } else {
        Err(CmdError::silent(status))
    }
}

/// The class a product operation's failure states, read from the typed
/// errors in its cause chain and never from its wording: the first operating
/// system error decides by its kind, a JSON document that does not parse or
/// fit is refused input, and a chain with neither states nothing.
fn product_failure_code(error: &anyhow::Error) -> crate::primitives::failure::FailureCode {
    for cause in error.chain() {
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
