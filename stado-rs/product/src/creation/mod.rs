mod checkout;
mod protocol;
mod provider;
mod state;
use crate::{
    catalog,
    common::{emit, Arguments, Runtime},
    registry,
};
use anyhow::{bail, Context, Result};
use protocol::{Request, MAX_BYTES, SCHEMA_VERSION};
use serde_json::{json, Value};
use state::Journal;
use std::{fs::File, io::Read};

pub fn run(args: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(args);
    if !args.positional.is_empty() {
        bail!("create does not take positional arguments");
    }
    let request_file = args.optional("--request")?;
    let status = args.optional("--status")?;
    let resume = args.optional("--resume")?;
    if usize::from(request_file.is_some())
        + usize::from(status.is_some())
        + usize::from(resume.is_some())
        != 1
    {
        bail!("create requires exactly one of --request, --status or --resume");
    }
    if status.is_none() && !args.has("--allow-create") {
        bail!("authority_required: --allow-create is required; request content cannot grant repository creation");
    }
    let incoming: Option<Request> = if let Some(path) = request_file {
        let mut raw = Vec::new();
        File::open(path)?
            .take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut raw)?;
        if raw.len() > MAX_BYTES {
            bail!("product creation request exceeds its protocol bound");
        }
        let request: Request = serde_json::from_slice(&raw)?;
        request.validate()?;
        Some(request)
    } else {
        None
    };
    let id = incoming
        .as_ref()
        .map(|r| r.request_id.as_str())
        .or(status)
        .or(resume)
        .context("missing request identity")?;
    protocol::identifier(id)?;
    let journal = Journal::open(runtime)?;
    let response_key = format!("response/{id}");
    if status.is_some() {
        let response = journal
            .get(&response_key)?
            .with_context(|| format!("unknown product creation request {id}"))?;
        emit(&response)?;
        return Ok(i32::from(response["state"] == "blocked"));
    }
    let request_key = format!("request/{id}");
    let previous = journal.get(&request_key)?;
    let request_value = match incoming.as_ref() {
        Some(request) => serde_json::to_value(request)?,
        None => previous
            .clone()
            .with_context(|| format!("unknown product creation request {id}"))?,
    };
    if previous.as_ref().is_some_and(|old| old != &request_value) {
        bail!("request_id_conflict: product creation payload differs from its durable request");
    }
    let request: Request = serde_json::from_value(request_value.clone())?;
    request.validate()?;
    journal.put(&request_key, &request_value)?;
    let workspace = runtime.workspace.canonicalize()?;
    if !runtime.workspace.is_absolute() || workspace != runtime.workspace {
        bail!("WISENT_WORKSPACE must name the absolute canonical workspace");
    }
    let catalog_path = runtime.catalog.canonicalize()?;
    let authority = json!({"catalog": catalog_path, "workspace": workspace});
    let authority_key = format!("authority/{id}");
    if journal
        .get(&authority_key)?
        .is_some_and(|previous| previous != authority)
    {
        bail!("creation authority changed: resume must use the original catalog and workspace");
    }
    journal.put(&authority_key, &authority)?;
    let mut response = journal.get(&response_key)?.unwrap_or_else(|| json!({
        "schema_version": SCHEMA_VERSION, "request_id": id, "initiative_id": request.initiative_id,
        "product": request.product, "state": "prepared", "repositories": [], "checkouts": [], "error": null,
    }));
    if response["state"] != "provisioned" {
        if let Err(error) = progress(&journal, &request, &mut response, runtime) {
            response["state"] = json!("blocked");
            response["error"] = json!(format!("{error:#}"));
        }
        journal.put(&response_key, &response)?;
    }
    emit(&response)?;
    if response["state"] == "blocked" {
        eprintln!("Error: {}", response["error"]);
    }
    Ok(i32::from(response["state"] == "blocked"))
}

fn progress(
    journal: &Journal,
    request: &Request,
    response: &mut Value,
    runtime: &Runtime,
) -> Result<()> {
    let id = &request.request_id;
    let marker = format!("creation://{id}");
    let document = catalog::load(&runtime.catalog)?;
    let existing = document["products"]
        .as_array()
        .context("products must be an array")?
        .iter()
        .find(|row| row["id"] == request.product.id);
    if existing.is_some_and(|row| {
        !row["evidence"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == &marker))
    }) {
        bail!(
            "product {} already exists and belongs to another creation request",
            request.product.id
        );
    }
    let response_key = format!("response/{id}");
    response["state"] = json!("provisioning");
    response["error"] = Value::Null;
    journal.put(&response_key, response)?;
    let mut repositories = Vec::new();
    let mut checkouts = Vec::new();
    for row in &request.repositories {
        let key = format!("provider/{id}/{}", row.repository);
        journal.put(
            &key,
            &json!({"state": "dispatching", "repository": row.repository}),
        )?;
        let observed = provider::ensure(&row.repository, id, &request.product.description)?;
        journal.put(&key, &json!({"state": "observed", "result": observed}))?;
        repositories.push(observed);
        response["repositories"] = json!(repositories);
        journal.put(&response_key, response)?;
        checkouts.push(checkout::ensure(
            journal,
            id,
            &row.repository,
            &runtime.workspace,
        )?);
        response["checkouts"] = json!(checkouts);
        journal.put(&response_key, response)?;
    }
    if existing.is_none() {
        let mut args = vec![
            "registry".to_owned(),
            "add".to_owned(),
            "--id".to_owned(),
            request.product.id.clone(),
            "--name".to_owned(),
            request.product.name.clone(),
            "--description".to_owned(),
            request.product.description.clone(),
            "--status".to_owned(),
            "preview".to_owned(),
            "--visibility".to_owned(),
            "private".to_owned(),
            "--family".to_owned(),
            request.product.family.clone(),
            "--owner-repository".to_owned(),
            request.repositories[0].repository.clone(),
            "--evidence".to_owned(),
            marker.clone(),
        ];
        for evidence in &request.evidence_refs {
            args.extend(["--evidence".into(), evidence.clone()]);
        }
        for repository in &request.repositories {
            let surface = serde_json::to_value(&repository.surface)?;
            args.extend([
                "--surface".into(),
                format!(
                    "{}={}",
                    surface.as_str().context("invalid surface")?,
                    repository.repository
                ),
            ]);
        }
        let mut parsed = crate::cli::registry::command().try_get_matches_from(args)?;
        let (action, arguments) = parsed
            .remove_subcommand()
            .context("creation registry operation is missing")?;
        response["registration"] =
            registry::mutate(&action, Arguments::from_matches(arguments), runtime)?;
    }
    let observed = catalog::load(&runtime.catalog)?;
    let product = catalog::product(&observed, &request.product.id)?;
    if !product["evidence"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item == &marker))
    {
        bail!("product registry read-back does not contain this creation identity");
    }
    response["state"] = json!("provisioned");
    response["product"] = product.clone();
    response["detail"] = json!("Repositories and a preview identity exist. Implementation, real qualification, release, installation and first use have not been established.");
    Ok(())
}
