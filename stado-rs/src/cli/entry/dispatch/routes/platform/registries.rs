//! Where the `stado registry` verbs land.

use crate::cli::*;

pub(super) async fn dispatch(command: RegistryCommands) -> Result<(), CmdError> {
    match command {
        RegistryCommands::Validate { path } => registry::validate(path),
        RegistryCommands::Import { path, json } => registry::import(path, json).await,
        RegistryCommands::Push {
            path,
            if_generation,
            force,
            allow_empty_fleet,
            json,
        } => registry::push(path, force, allow_empty_fleet, if_generation, json).await,
        RegistryCommands::Pull {
            with_generation,
            generation_only,
            path,
        } => registry::pull(with_generation, generation_only, path.as_deref()).await,
        RegistryCommands::Set { path, value, json } => registry::set(&path, &value, json).await,
        RegistryCommands::SelfTarget { name_only } => registry::self_target(name_only).await,
        RegistryCommands::Doctor { json } => registry::doctor(json).await,
        RegistryCommands::Host(command) => match command {
            RegistryHostCommands::Show { host, path } => {
                registry::host_show(&host, path.as_deref()).await
            }
            RegistryHostCommands::Add {
                host,
                ssh,
                kind,
                release_platform,
            } => registry::host_add(&host, &ssh, &kind, &release_platform).await,
            // A caller that asked for a typed receipt gets a typed
            // refusal: Stado Desktop reads these documents and cannot
            // handle a prose failure where a receipt was promised.
            RegistryHostCommands::Path { command } => match command {
                RegistryHostPathCommands::List { host, json } => {
                    registry::host_path_list(&host, json)
                        .await
                        .map_err(|error| error.machine_readable(json))
                }
                RegistryHostPathCommands::Set {
                    host,
                    path,
                    ssh,
                    priority,
                    json,
                } => registry::host_path_set(&host, &path, &ssh, priority, json)
                    .await
                    .map_err(|error| error.machine_readable(json)),
                RegistryHostPathCommands::Remove { host, path, json } => {
                    registry::host_path_remove(&host, &path, json)
                        .await
                        .map_err(|error| error.machine_readable(json))
                }
            },
        },
        RegistryCommands::BeaconAge { json } => registry::beacon_age(json).await,
    }
}
