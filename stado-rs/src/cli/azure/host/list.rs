//! `stado cloud host list --provider azure`: every cloud host the registry
//! declares, with what Azure says of its machine now, its priced hour, and
//! the hourly burn of all of them.

use serde_json::json;

use super::declaration;
use super::{azure, ProviderArgs};
use crate::cli::CmdError;
use crate::providers::azure::{cloud_host, power_state};

pub(super) async fn list(args: ProviderArgs) -> Result<(), CmdError> {
    let super::super::CloudProvider::Azure = args.provider;
    let az = azure()?;
    let (document, _) = crate::cli::registry::fetch_versioned_document().await?;
    let hosts = declaration::hosts(&document, super::create::PROVIDER)?;
    let mut rows = Vec::new();
    let mut burn = 0.0;
    for host in &hosts {
        let machine = match cloud_host::read(&az.client, az.resource_group, &host.name).await {
            Ok(Some(vm)) => match power_state(&vm) {
                Some(state) => state,
                None => "unreported".to_string(),
            },
            Ok(None) => "absent".to_string(),
            Err(cause) => format!("unread: {cause}"),
        };
        if machine == "running" {
            burn += host.cloud.hourly_usd;
        }
        if !args.json {
            println!(
                "{}  {machine}  {} {}  {}  {:.2} USD/h  idle limit {} min  since {}",
                host.name,
                host.cloud.location,
                host.cloud.size,
                host.cloud.address,
                host.cloud.hourly_usd,
                host.cloud.idle_minutes,
                host.cloud.created_at
            );
        }
        rows.push(json!({ "target": host.name, "machine": machine, "cloud": host.cloud }));
    }
    if args.json {
        // `Value` indexing reads an absent section as null, which is what an
        // admission never measured is.
        let egress = document["cloud_hosts"]["egress"].clone();
        let answer = json!({ "hosts": rows, "running_hourly_usd": burn, "egress": egress });
        return crate::cli::print_answer(&answer, true);
    }
    if hosts.is_empty() {
        println!("the registry declares no Azure cloud host");
    } else {
        println!("running burn: {burn:.2} USD/h");
    }
    Ok(())
}
