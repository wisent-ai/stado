mod checkout;
mod plan;
mod protocol;
mod provider;
mod state;
pub mod surface;
use crate::{
    catalog,
    common::{emit, Arguments, Runtime},
    registry,
};
use anyhow::{bail, Context, Result};
pub use protocol::Surface;
use protocol::{distinct::Proposed, Request, SCHEMA_VERSION};
use serde_json::{json, Value};
use state::Journal;
use std::path::Path;

pub fn run(args: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(args);
    if !args.positional.is_empty() {
        bail!("create does not take positional arguments");
    }
    let request_file = args.optional("--request")?;
    let plan_file = args.optional("--plan")?;
    let status = args.optional("--status")?;
    let resume = args.optional("--resume")?;
    if usize::from(request_file.is_some())
        + usize::from(plan_file.is_some())
        + usize::from(status.is_some())
        + usize::from(resume.is_some())
        != 1
    {
        bail!("create requires exactly one of --request, --plan, --status or --resume");
    }
    if let Some(path) = plan_file {
        return plan::create(&read(path)?, runtime, args.has("--json"));
    }
    if status.is_none() && !args.has("--allow-create") {
        bail!("authority_required: --allow-create is required; request content cannot grant repository creation; `--plan` shows what the request would create without it");
    }
    let incoming = request_file.map(read).transpose()?;
    let id = match (&incoming, status, resume) {
        (Some(request), _, _) => request.request_id.as_str(),
        (None, Some(id), _) => {
            protocol::identifier("--status", id)?;
            id
        }
        (None, None, Some(id)) => {
            protocol::identifier("--resume", id)?;
            id
        }
        (None, None, None) => bail!("missing request identity"),
    };
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
    let authority = authority(runtime)?;
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

/// The creation request in the file at `path`, refused naming the field.
fn read(path: &str) -> Result<Request> {
    // The request is the caller's own file, read whole; its schema, not a
    // byte count, decides whether it is acceptable.
    let raw = std::fs::read(path).with_context(|| format!("reading creation request {path}"))?;
    Request::parse(&raw).with_context(|| format!("creation request {path}"))
}

/// The catalog and workspace a request is carried out against; a resumed
/// request must use the same.
fn authority(runtime: &Runtime) -> Result<Value> {
    let workspace = runtime.workspace.canonicalize().with_context(|| {
        format!(
            "WISENT_WORKSPACE {} cannot be read",
            runtime.workspace.display()
        )
    })?;
    if !runtime.workspace.is_absolute() || workspace != runtime.workspace {
        bail!(
            "WISENT_WORKSPACE must name the absolute canonical workspace; {} resolves to {}",
            runtime.workspace.display(),
            workspace.display()
        );
    }
    let catalog = runtime
        .catalog
        .canonicalize()
        .with_context(|| format!("catalog {} cannot be read", runtime.catalog.display()))?;
    Ok(json!({"catalog": catalog, "workspace": workspace}))
}

/// The marker a product registered by request `id` carries in `evidence`.
fn marker(id: &str) -> String {
    format!("creation://{id}")
}

/// The catalog record of `request.product`, if one exists; a record that
/// another request (or no request) registered refuses this one.
fn existing<'a>(document: &'a Value, request: &Request) -> Result<Option<&'a Value>> {
    let marker = marker(&request.request_id);
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
            "product.id: product {} already exists in the catalog and no creation request {} \
             registered it; add a surface to it with `stado product surface add` instead",
            request.product.id,
            request.request_id
        );
    }
    Ok(existing)
}

/// Each repository of `request` with the field that names it.
fn proposed(request: &Request) -> Vec<Proposed<'_>> {
    request
        .repositories
        .iter()
        .enumerate()
        .map(|(index, row)| Proposed {
            field: format!("repositories[{index}].repository"),
            repository: &row.repository,
        })
        .collect()
}

/// The `registry add` arguments that register `request` as a preview
/// product.
fn registration(request: &Request) -> Result<Arguments> {
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
        request.product.family.to_string(),
        "--owner-repository".to_owned(),
        request.repositories[0].repository.clone(),
        "--evidence".to_owned(),
        marker(&request.request_id),
    ];
    for evidence in &request.evidence_refs {
        args.extend(["--evidence".into(), evidence.clone()]);
    }
    for repository in &request.repositories {
        args.extend([
            "--surface".into(),
            format!("{}={}", repository.surface, repository.repository),
        ]);
    }
    registry_arguments(args)
}

/// `args` parsed as the `stado product registry` operation they name.
fn registry_arguments(args: Vec<String>) -> Result<Arguments> {
    let mut parsed = crate::cli::registry::command().try_get_matches_from(args)?;
    let (_, arguments) = parsed
        .remove_subcommand()
        .context("creation registry operation is missing")?;
    Ok(Arguments::from_matches(arguments))
}

/// Provision `repository` for request `id` and its canonical checkout,
/// journaling each step so a resumed request repeats none of it.
fn provision(
    journal: &Journal,
    id: &str,
    repository: &str,
    description: &str,
    workspace: &Path,
) -> Result<(Value, Value)> {
    let key = format!("provider/{id}/{repository}");
    journal.put(
        &key,
        &json!({"state": "dispatching", "repository": repository}),
    )?;
    let observed = provider::ensure(repository, id, description)?;
    journal.put(&key, &json!({"state": "observed", "result": observed}))?;
    let checkout = checkout::ensure(journal, id, repository, workspace)?;
    Ok((observed, checkout))
}

fn progress(
    journal: &Journal,
    request: &Request,
    response: &mut Value,
    runtime: &Runtime,
) -> Result<()> {
    let id = &request.request_id;
    let document = catalog::load(&runtime.catalog)?;
    let existing = existing(&document, request)?.is_some();
    // A new product is new only if what it would create does not exist yet:
    // checked before the first repository is provisioned.
    if !existing {
        protocol::distinct::check(
            Some(journal),
            id,
            Some(("product.id", &request.product.id)),
            &proposed(request),
            &document,
        )?;
    }
    let response_key = format!("response/{id}");
    response["state"] = json!("provisioning");
    response["error"] = Value::Null;
    journal.put(&response_key, response)?;
    let mut repositories = Vec::new();
    let mut checkouts = Vec::new();
    for row in &request.repositories {
        let (observed, checkout) = provision(
            journal,
            id,
            &row.repository,
            &request.product.description,
            &runtime.workspace,
        )?;
        repositories.push(observed);
        checkouts.push(checkout);
        response["repositories"] = json!(repositories);
        response["checkouts"] = json!(checkouts);
        journal.put(&response_key, response)?;
    }
    if !existing {
        response["registration"] = registry::mutate("add", registration(request)?, runtime)?;
    }
    let observed = catalog::load(&runtime.catalog)?;
    let product = catalog::product(&observed, &request.product.id)?;
    if !product["evidence"]
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item == &marker(id)))
    {
        bail!("product registry read-back does not contain this creation identity");
    }
    response["state"] = json!("provisioned");
    response["product"] = product.clone();
    response["detail"] = json!("Repositories and a preview identity exist. Implementation, real qualification, release, installation and first use have not been established.");
    Ok(())
}
