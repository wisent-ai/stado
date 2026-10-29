//! `stado cloud host create NAME --provider azure …`: price the machine
//! against the credit floor, create it reachable only from the fleet's own
//! addresses, enroll it, and see its toolchain answer.

use serde_json::json;

use super::declaration::{self, Cloud};
use super::{azure, egress, reap, CreateArgs};
use crate::cli::registry::{commit_document, fetch_versioned_document};
use crate::cli::CmdError;
use crate::providers::azure::{cloud_host, credit};

/// The first-boot toolchain; `@USER@` is the deployment's VM user.
const CLOUD_INIT: &str = include_str!("cloud-init.yaml");

/// The provider word a cloud declaration records.
pub(super) const PROVIDER: &str = "azure";

/// What the host is asked once cloud-init has finished: the toolchain
/// `stado host cargo` builds with, each tool by the path its candidates name.
const TOOLCHAIN_PROBE: &str = "set -e\n\
    \"$HOME/.cargo/bin/cargo\" --version\n\
    node --version\n\
    npm --version\n";

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub(super) async fn create(args: CreateArgs) -> Result<(), CmdError> {
    let super::super::CloudProvider::Azure = args.provider;
    if !valid_name(&args.name) {
        return Err(CmdError::usage(format!(
            "{:?} is not a target name: lowercase letters, digits and '-', not starting with '-'",
            args.name
        )));
    }
    if args.disk_gb <= 0 || args.idle_minutes <= 0 {
        return Err(CmdError::usage(
            "--disk-gb and --idle-minutes must each be a whole number above zero",
        ));
    }
    let say = |line: &str| {
        if args.json {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    };
    let az = azure()?;
    let name = args.name.as_str();

    // 1. The registry: no host of this name, who reaches cloud hosts, and
    //    the burn of the cloud hosts already running.
    let (document, _) = fetch_versioned_document().await?;
    if document["targets"].as_array().is_some_and(|targets| {
        targets
            .iter()
            .any(|target| target["name"].as_str() == Some(name))
    }) {
        return Err(CmdError::refused(format!(
            "target '{name}' is already registered"
        )));
    }
    let admit_from = declaration::admit_from(&document)?;
    let running = declaration::hosts(&document, PROVIDER)?;
    // Folded from +0.0: `Sum` for f64 starts at -0.0, which an empty fleet
    // printed as "-0.00 USD".
    let running_hourly = running
        .iter()
        .fold(0.0, |burn, host| burn + host.cloud.hourly_usd);

    // 2. The credit gate, and the burn it decided on, said before anything
    //    is created.
    let operation = format!(
        "creating cloud host {name} ({}) in {}",
        args.size, args.location
    );
    // The gate's refusal states the rule that refused, so it is a refusal,
    // not an unattributed failure.
    let admitted = credit::admit_burn(&operation, &args.size, running_hourly)
        .await
        .map_err(|error| CmdError::refused(error.to_string()))?;
    say(&format!(
        "projected hourly burn: {:.2} USD ({name} {} at {:.2} USD, {}; {} cloud host(s) already running at {:.2} USD)",
        admitted.hourly + running_hourly,
        args.size,
        admitted.hourly,
        admitted.priced,
        running.len(),
        running_hourly,
    ));
    say(&format!(
        "credit: balance {:.2} USD, {:.2} USD after one hour of all of them, above the floor {}",
        admitted.balance,
        admitted.after_hour(),
        admitted.floor.named()
    ));

    // 3. The fleet's addresses, recorded and admitted.
    let measured = egress::measure(&admit_from).await?;
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    commit_document(|document| Ok(declaration::with_egress(document, &measured, &at))).await?;
    let sources = egress::sources(&measured);
    say(&format!(
        "admitting {} ({})",
        sources.join(", "),
        measured
            .iter()
            .map(|(host, address)| format!("{host} {address}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    reap::readmit(&az, &running, &sources).await?;
    let admission = cloud_host::admit(
        &az.client,
        az.resource_group,
        az.vnet,
        az.subnet,
        &args.location,
        &sources,
    )
    .await
    .map_err(CmdError::refused)?;

    // 4. The host's own fleet key: the VM is created holding its public half.
    let runner = crate::deploy::production_runner();
    let (public_key, fingerprint) =
        crate::cli::fleet::key::rotate::generate_stored(&runner, name).await?;
    say(&format!(
        "minted fleet key {} ({fingerprint})",
        crate::cli::fleet::key::item_id(name)
    ));

    // 5. The machine.
    let custom_data = CLOUD_INIT.replace("@USER@", az.username);
    let spec = cloud_host::Spec {
        name,
        location: &args.location,
        size: &args.size,
        disk_gb: args.disk_gb,
        image_urn: az.image_urn,
        username: az.username,
        public_key: public_key.trim(),
        custom_data: &custom_data,
        subnet_vnet: az.vnet,
        subnet: az.subnet,
    };
    let created = match cloud_host::create(
        &az.client,
        az.resource_group,
        &spec,
        &sources,
        &admission,
    )
    .await
    {
        Ok(created) => created,
        Err(error) => {
            let removed = super::delete::forget_key(name).await;
            return Err(CmdError::refused(match removed {
                Ok(_) => error,
                Err(key) => format!("{error}; its fleet key could not be removed either: {key}"),
            }));
        }
    };
    say(&format!(
        "created {name} at {} in {}",
        created.address, args.location
    ));

    // 6. The verified registration, then its cloud declaration. A host that
    //    cannot be enrolled is deleted again: nothing would reap it.
    let destination = format!("{}@{}", az.username, created.address);
    let enrolled = crate::cli::fleet::ops::enrolled(
        crate::cli::fleet::ops::EnrollRequest {
            name,
            ssh: &destination,
            kind: "local",
            fleet: None,
            bootstrap: false,
            install_key: false,
            shared: false,
        },
        args.json,
    )
    .await;
    if let Err(error) = enrolled {
        let deleted = cloud_host::delete(&az.client, az.resource_group, name).await;
        let removed = super::delete::forget_key(name).await;
        let mut message = format!(
            "{name} was created at {} but could not be enrolled: {error}",
            created.address
        );
        match deleted {
            Ok(_) => message.push_str(&format!("; {name} was deleted again")),
            Err(standing) => message.push_str(&format!(
                "; these are still standing and billing: {}",
                standing.join("; ")
            )),
        }
        if let Err(key) = removed {
            message.push_str(&format!("; its fleet key could not be removed: {key}"));
        }
        return Err(CmdError::unreachable(message));
    }
    let cloud = Cloud {
        provider: PROVIDER.to_string(),
        location: args.location.clone(),
        size: args.size.clone(),
        disk_gb: args.disk_gb,
        hourly_usd: admitted.hourly,
        priced: admitted.priced.clone(),
        idle_minutes: args.idle_minutes,
        address: created.address.clone(),
        vm_id: created.vm_id.clone(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    let generation =
        commit_document(|document| declaration::with_cloud(document, name, &cloud)).await?;
    say(&format!(
        "registered {name} as {destination} (generation {generation})"
    ));

    // 7. First boot to its end, and the toolchain it installed.
    let target = crate::deploy::host_channel::canonical_target(name).await?;
    let booted = crate::deploy::host_channel::run_script(
        &target,
        "exec cloud-init status --wait --long\n",
        &runner,
    )
    .await?;
    if !booted.succeeded() {
        return Err(CmdError::unreachable(format!(
            "{name} is registered, but its first boot did not finish cleanly (cloud-init exited {}): {}; \
             /var/log/cloud-init-output.log on {name} says why, and `stado cloud host delete {name} --provider azure` returns it",
            booted.code,
            booted.stdout.trim()
        )));
    }
    let toolchain =
        crate::deploy::host_channel::run_script(&target, TOOLCHAIN_PROBE, &runner).await?;
    if !toolchain.succeeded() {
        return Err(CmdError::unreachable(format!(
            "{name} finished its first boot, but its toolchain does not answer: {}",
            toolchain.detail().trim()
        )));
    }
    let receipt = json!({
        "target": name,
        "ssh": destination,
        "cloud": cloud,
        "projected_hourly_usd": admitted.hourly + running_hourly,
        "balance_usd": admitted.balance,
        "after_one_hour_usd": admitted.after_hour(),
        "floor": { "amount": admitted.floor.amount, "currency": admitted.floor.currency, "ruling": admitted.floor.ruling },
        "admitted": measured.iter().map(|(host, address)| json!({ "host": host, "address": address })).collect::<Vec<_>>(),
        "toolchain": toolchain.stdout.lines().collect::<Vec<_>>(),
        "generation": generation,
    });
    if args.json {
        crate::cli::print_answer(&receipt, true)?;
    } else {
        for line in toolchain.stdout.lines() {
            println!("{name}: {line}");
        }
        println!(
            "{name} is a fleet host: `stado host cargo {name} build --manifest-path <crate>/Cargo.toml --revision <commit> --bin <bin>` builds there"
        );
    }
    Ok(())
}
