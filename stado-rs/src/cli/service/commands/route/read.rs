//! Where the read and host-inspection verbs land.

use super::*;

use crate::cli::service::lifecycle::deploy::catalog::catalog;
use crate::cli::service::reports::list::{list, list_undeclared, list_unowned};
use crate::cli::service::reports::probe::bootout;
use crate::cli::service::reports::probe::label_print::label_print;
use crate::cli::service::reports::probe::reap::reap;
use crate::cli::service::reports::probe::watch_spawn::watch_spawn;
use crate::cli::service::reports::status::status;
use crate::cli::service::reports::view::onboarding_catalog;

use super::super::spec::read::ReadCommands;

pub(crate) async fn dispatch(command: ReadCommands) -> Result<(), CmdError> {
    match command {
        ReadCommands::Directory(sub) => crate::cli::directory::dispatch(sub).await,
        ReadCommands::Catalog { json } => catalog(json).await,
        ReadCommands::List {
            unowned,
            undeclared,
            json,
        } => {
            if unowned && undeclared {
                return Err(CmdError::usage(
                    "--unowned and --undeclared are two different questions; ask one at a time",
                ));
            }
            if undeclared {
                list_undeclared(json).await
            } else if unowned {
                list_unowned(json).await
            } else {
                list(json).await
            }
        }
        ReadCommands::Bootout {
            label,
            host,
            domain,
            json,
        } => bootout(&label, &host, domain.as_deref(), json).await,
        ReadCommands::Reap {
            host,
            command,
            apply,
            json,
        } => reap(&host, &command, apply, json).await,
        ReadCommands::WatchSpawn {
            host,
            command,
            seconds,
            interval_ms,
            json,
        } => watch_spawn(&host, &command, seconds, interval_ms, json).await,
        ReadCommands::LabelPrint {
            label,
            host,
            domain,
            json,
        } => label_print(&label, &host, domain.as_deref(), json).await,
        ReadCommands::Verify { host, local, json } => {
            if local {
                crate::cli::service_verify::verify_local(json).await
            } else {
                crate::cli::service_verify::verify(host.as_deref(), json).await
            }
        }
        ReadCommands::Converge {
            target,
            binary,
            apply,
            json,
        } => crate::cli::service_converge::converge(&target, binary.as_deref(), apply, json).await,
        ReadCommands::OnboardingCatalog => onboarding_catalog().await,
        ReadCommands::Status { name, json } => status(&name, json).await,
    }
}
