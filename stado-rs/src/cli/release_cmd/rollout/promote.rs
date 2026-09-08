//! Promoting exact verified candidate bytes into the registry's desired
//! state, with the channel and rollout-generation arithmetic that implies.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use serde_json::json;

use crate::cli::release_cmd::verified_artifact;
use crate::cli::CmdError;
use crate::release_control::{self, DesiredRelease, ProductReleasePolicy, ReleaseChannel};

use super::{ChannelArg, ReleasePromoteArgs};

fn platforms(policy: &ProductReleasePolicy) -> BTreeSet<String> {
    policy
        .targets
        .values()
        .map(|target| target.platform.clone())
        .collect()
}

pub(in crate::cli::release_cmd) async fn promote(
    args: &ReleasePromoteArgs,
    exact_is_noop: bool,
) -> Result<(), CmdError> {
    let mut last_conflict = None;
    for attempt in 0..3 {
        let (document, expected_generation) =
            crate::cli::registry::fetch_versioned_document().await?;
        let mut control = release_control::control(&document)?
            .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
        let policy = control.products.get(&args.product).ok_or_else(|| {
            CmdError::click(format!("unknown release product {:?}", args.product))
        })?;
        let mut artifacts = BTreeMap::new();
        let mut revisions = BTreeSet::new();
        for platform in platforms(policy) {
            let artifact =
                verified_artifact(&args.product, &args.version, &platform, &control).await?;
            revisions.insert(artifact.source_revision.clone());
            artifacts.insert(platform, artifact);
        }
        if revisions.len() != 1 {
            return Err(CmdError::click(
                "release platforms were not built from one source revision",
            ));
        }
        let channel = ReleaseChannel::from(args.channel);
        if exact_is_noop {
            if let Some(desired) = policy.desired.as_ref().filter(|desired| {
                desired.version == args.version
                    && desired.channel == channel
                    && desired.artifacts == artifacts
            }) {
                let report = json!({
                    "product": args.product,
                    "version": args.version,
                    "channel": channel,
                    "rollout_generation": desired.rollout_generation,
                    "registry_generation": control.generation,
                    "store_generation": expected_generation,

                });
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!(
                        "already promoted {} {} channel={:?} rollout-generation={} registry-generation={}",
                        args.product,
                        args.version,
                        channel,
                        desired.rollout_generation,
                        control.generation
                    );
                }
                return Ok(());
            }
        }
        let policy = control
            .products
            .get_mut(&args.product)
            .expect("checked above");
        let rollout_generation = policy
            .desired
            .as_ref()
            .map_or(1, |desired| desired.rollout_generation.saturating_add(1));
        policy.previous = policy.desired.take();
        policy.desired = Some(DesiredRelease {
            version: args.version.clone(),
            channel,
            rollout_generation,
            promoted_at: Utc::now().to_rfc3339(),
            artifacts,
        });
        control.generation = control.generation.saturating_add(1);
        let mut updated = document;
        updated[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&control)?;
        let stored_generation =
            match crate::cli::registry::push_document_if(&updated, &expected_generation).await {
                Ok(generation) => generation,
                Err(error)
                    if error
                        .to_string()
                        .contains("storage version changed for registry.json") =>
                {
                    last_conflict = Some(error);
                    if attempt < 2 {
                        tokio::time::sleep(std::time::Duration::from_millis(100 * (attempt + 1)))
                            .await;
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };
        let report = json!({
            "product": args.product,
            "version": args.version,
            "channel": channel,
            "rollout_generation": rollout_generation,
            "registry_generation": control.generation,
            "store_generation": stored_generation,
        });
        if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "promoted {} {} channel={:?} rollout-generation={} registry-generation={}",
                args.product, args.version, channel, rollout_generation, control.generation
            );
        }
        return Ok(());
    }
    Err(last_conflict
        .unwrap_or_else(|| CmdError::click("release promotion exhausted registry retries")))
}

pub(crate) async fn promote_for_submit(
    product: &str,
    version: &str,
    channel: crate::release_pipeline::PipelineChannel,
) -> Result<(), CmdError> {
    promote(
        &ReleasePromoteArgs {
            product: product.to_string(),
            version: version.to_string(),
            channel: match channel {
                crate::release_pipeline::PipelineChannel::Candidate => ChannelArg::Candidate,
                crate::release_pipeline::PipelineChannel::Stable => ChannelArg::Stable,
            },
            json: false,
        },
        true,
    )
    .await
}
