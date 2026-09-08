//! The command itself: the stages in `stages/` driven in the one order that
//! leaves no half-configured unit behind.

use serde_json::{json, Map};

use crate::cli::web::{product, unit_label, UNIT_DOMAIN};
use crate::cli::CmdError;
use crate::declaration::{DeclarationRun, DeclarationSource, ServiceDeclaration};
use crate::deploy::{host_channel, production_runner, service};

use super::record::record_declaration;
use super::stages::environment::{
    ensure_bearer, grant_capabilities, secret_deliveries, unit_environment,
};
use super::stages::install::{install_release, launcher_program};
use super::stages::readiness::wait_until_ready;
use super::stages::release::published_stable_version;
use super::{click, GRANT_TTL_SECONDS, VAULT_FILE, WEB_ENV_DIR, WEB_PLATFORM, WEB_TOKEN_DIR};

pub(crate) async fn deploy(name: &str, version: Option<&str>, json: bool) -> Result<(), CmdError> {
    let declared = product(name)?;
    // A redirect lives entirely in the edge's configuration: no release, no
    // tarball, no unit. Refusing here names that, rather than failing later
    // on a host name the declaration does not carry.
    if declared.is_redirect() {
        return Err(CmdError::click(format!(
            "web product {name} is a redirect to {}, so there is nothing to deploy: `stado web route {name}` is what publishes it",
            declared.redirect_to().unwrap_or_default()
        )));
    }
    // A hostname in front of an existing service owns no release either. The
    // service was declared, deployed and is kept running by whoever owns it —
    // Brama is a Rust binary with its own unit, not a Node web product — and
    // installing a web release over it is the one thing this must never do.
    if let Some(service) = declared.upstream_service() {
        return Err(CmdError::click(format!(
            "web product {name} is a hostname in front of the registry service {service:?}, so there is nothing here to deploy: that service has its own unit and its own release. `stado web route {name}` publishes the hostname, `stado service status {service}` reports the unit, and `stado service deploy` is what installs it"
        )));
    }
    let host = declared.host();
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let runner = production_runner();

    // The exact coordinate first, before anything is touched. An operator who
    // named `--version` gets that version and no resolution at all; without it
    // the release plane answers, and a product with nothing published is
    // refused here rather than after a host has been changed.
    let version = match version {
        Some(version) => version.to_string(),
        None => published_stable_version(name).await?,
    };
    // The signed release manifest is the only source of the digest a host must
    // reproduce, and this is the function every other consumer of a pipeline
    // release reads it through: it validates the manifest's immutable
    // identity, checks qualification, verifies the signature against a key
    // the registry trusts, and confirms the archive matches. A release that
    // fails any of those never reaches a host.
    let artifact =
        crate::cli::release_cmd::verified_artifact_for_submit(name, &version, WEB_PLATFORM).await?;

    // Every operator-facing path resolves against the host's own account, not
    // this machine's: a target's home is its own business.
    let home = host_channel::remote_home(&target, &runner)
        .await
        .map_err(click)?;
    let env_file = format!("{home}/{WEB_ENV_DIR}/{name}.env");
    let token_file = format!("{home}/{WEB_TOKEN_DIR}/{name}.token");
    let program = launcher_program(&target, &home, name);
    let environment = unit_environment(declared, &env_file);

    // Refuse a malformed secret reference and an unauthorized database before
    // the release lands, so a product whose declaration cannot be satisfied
    // does not leave a half-configured unit behind.
    let deliveries = secret_deliveries(declared)?;

    let version_directory = install_release(
        &target,
        name,
        &version,
        &artifact.archive_uri,
        &artifact.artifact_sha256,
        &runner,
    )
    .await?;

    // The unit itself, rendered and installed by the same engine
    // `stado service ensure` uses. `ensure` rather than `deploy`, because
    // `stado web deploy` is how every subsequent release lands too: it
    // installs the unit where the host does not have it, leaves matching
    // loaded definitions alone, and reloads only an actual definition drift.
    let label = unit_label(name);
    let plan = service::plan_deploy_labelled(&target, name, &label, &program, &[], &environment)
        .map_err(click)?;
    let outcome = service::ensure_service(&target, &plan, &runner)
        .await
        .map_err(click)?;
    if !outcome.succeeded() {
        return Err(CmdError::click(format!(
            "{host}: could not install the unit {label}: {}",
            outcome.report.failure()
        )));
    }
    let record = service::record_from_ensure(host, name, &outcome, &now());

    // The grant before the secrets: the unit's Skarbiec identity is what
    // authorizes it to hold them, and minting it afterwards would leave a
    // window in which the values are on the host and nothing says who may
    // read them.
    //
    // A product that declares no secrets and no database needs no grant, and
    // must not be given an empty one: Skarbiec refuses `token-mint` without
    // capabilities (`token-mint requires --capabilities action:item[#field]`),
    // so minting unconditionally made a static site undeployable —
    // `preferences-landing`, which reads nothing, could not be installed at
    // all. A consumer with nothing to read is the correct end state for it,
    // and the narrowest one: no bearer, no capability, nothing to leak.
    let capabilities = grant_capabilities(&deliveries);
    let bearer = if capabilities.is_empty() {
        "none: this product declares no secrets".to_string()
    } else {
        let bearer = ensure_bearer(&target, &token_file, &runner).await?;
        let grant = service::remint_consumer_grant_on_host(
            &target,
            declared.consumer(),
            &capabilities,
            &token_file,
            VAULT_FILE,
            GRANT_TTL_SECONDS,
            declared.consumer(),
            &runner,
        )
        .await
        .map_err(click)?;
        if !grant.succeeded("grant_synced") {
            return Err(CmdError::click(format!(
                "{host}: could not mint the Skarbiec grant for consumer {}: {}",
                declared.consumer(),
                grant.failure()
            )));
        }
        bearer
    };

    // One field of one item into one variable, over the host channel, for
    // every entry. The value is read through the isolated service-verifier
    // grant, travels only inside the channel's request body, and is dropped
    // the moment it has been written: nothing below ever prints, logs or
    // returns it, and only the variable NAMES reach the report.
    let mut delivered: Vec<String> = Vec::with_capacity(deliveries.len());
    for (variable, item, field) in &deliveries {
        let secret = crate::cli::service::service_secret(item, field).await?;
        let synced =
            service::sync_service_secret(&target, &record, &env_file, variable, &secret, &runner)
                .await
                .map_err(click)?;
        drop(secret);
        if !synced.succeeded("secret_synced") {
            return Err(CmdError::click(format!(
                "{host}: could not deliver {variable} into {env_file}: {}",
                synced.failure()
            )));
        }
        delivered.push(variable.clone());
    }

    // Restart unconditionally, and only now. The program path is identical
    // across releases — it goes through `current` on purpose, so a new
    // release moves every unit forward without re-rendering any of them — so
    // `ensure` correctly reports a unit already running the declared program
    // as already correct and touches nothing. That is exactly the case where
    // "already running the declared program" is not "already correct": the
    // program is the same file and both the bytes behind `current` and the
    // env file it sources have changed underneath it.
    let restarted = service::restart_service(&target, &record, &runner)
        .await
        .map_err(click)?;
    if !restarted.succeeded("restarted") {
        return Err(CmdError::click(format!(
            "{host}: {label} did not restart: {}",
            restarted.failure()
        )));
    }

    let readyz_url = format!("http://127.0.0.1:{}{}", declared.port(), declared.readyz());
    let (readiness, readiness_detail) = wait_until_ready(&target, &readyz_url, &runner).await?;
    if readiness != "ready" {
        return Err(CmdError::click(format!(
            "{host}: {label} never answered 200 on port {} at {} — {readiness_detail}. The unit \
             is installed and its environment is delivered; `stado service logs {name} --host \
             {host}` is where the reason is.",
            declared.port(),
            declared.readyz(),
        )));
    }

    let declaration = ServiceDeclaration {
        source: DeclarationSource {
            artifact: artifact.archive_uri.clone(),
            sha256: artifact.artifact_sha256.clone(),
            extra: Map::new(),
        },
        run: DeclarationRun {
            program: Some(program.clone()),
            args: Vec::new(),
            env: environment.iter().cloned().collect(),
            extra: Map::new(),
        },
        extra: Map::new(),
    };
    let generation = record_declaration(name, declared, &record, &declaration).await?;

    // The database credential is appended last by `secret_deliveries`, so it
    // is that list's final entry when the product declares one. Read from
    // there rather than resolved a second time: two resolutions of the same
    // consumer against the same declaration are two answers that can differ.
    let database_item = declared
        .database()
        .and_then(|_| deliveries.last().map(|(_, item, _)| item.clone()));
    let report = json!({
        "product": name,
        "host": host,
        "unit": record.unit_id(),
        "unit_domain": UNIT_DOMAIN,
        "port": declared.port(),
        "version": &version,
        "artifact": &artifact.archive_uri,
        "artifact_sha256": &artifact.artifact_sha256,
        "version_directory": &version_directory,
        "program": &program,
        "consumer": declared.consumer(),
        "bearer": &bearer,
        "capabilities": &capabilities,
        "env_file": &env_file,
        "variables": &delivered,
        "database_item": &database_item,
        "readiness": &readiness,
        "readiness_detail": &readiness_detail,
        "readiness_url": &readyz_url,
        "unit_action": &outcome.action,
        "registry_generation": &generation,
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{name}: {} on {host} port {} running {version} (sha256 {})",
            record.unit_id(),
            declared.port(),
            artifact.artifact_sha256,
        );
        println!("  program:   {program}");
        println!("  consumer:  {} (bearer {bearer})", declared.consumer());
        println!(
            "  variables: {}",
            if delivered.is_empty() {
                "none declared".to_string()
            } else {
                delivered.join(", ")
            }
        );
        if let Some(database) = declared.database() {
            println!(
                "  database:  {} field {} as {} from item {}",
                database.name(),
                database.field(),
                database.variable(),
                database_item.as_deref().unwrap_or("-"),
            );
        }
        println!("  readiness: {readiness} — {readiness_detail} at {readyz_url}");
    }
    Ok(())
}

/// `datetime.now(timezone.utc).isoformat()`, as every other writer in the
/// crate stamps a managed-service record.
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
