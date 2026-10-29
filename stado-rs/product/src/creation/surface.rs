//! `stado product surface add PRODUCT --kind KIND --repository OWNER/NAME`:
//! provision one more private repository for a product the catalog already
//! holds and record it as that product's KIND surface. It takes the path
//! `stado product create` takes for each repository (the same refusals of
//! what exists, the same Wisent Integrations call, the same canonical
//! checkout and journal) and ends with `registry set --add-surface`.
//! `--plan` carries out every check with reads only and prints what would be
//! created; `--allow-create` creates it. A run that stopped is resumed by
//! running it again: its journal identity is `surface-PRODUCT-KIND`.

use super::{
    authority,
    plan::{Integration, Plan, Registration, Row},
    protocol::{self, distinct::Proposed},
    provision, registry_arguments,
    state::Journal,
    Surface,
};
use crate::{
    catalog,
    common::{emit, Arguments, Runtime},
    registry,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// The `registry set` arguments that record `kind=repository` on `product`.
fn registration(product: &str, kind: Surface, repository: &str) -> Result<Arguments> {
    registry_arguments(vec![
        "registry".to_owned(),
        "set".to_owned(),
        product.to_owned(),
        "--add-surface".to_owned(),
        format!("{kind}={repository}"),
    ])
}

/// Refuse a request `id` the journal holds as started for another
/// repository or against another catalog and workspace.
fn started(
    journal: &Journal,
    id: &str,
    (product, kind, repository): (&str, Surface, &str),
    authority: &Value,
) -> Result<()> {
    let requested = json!({"product": product, "kind": kind, "repository": repository});
    if let Some(previous) = journal
        .get(&format!("surface/{id}"))?
        .filter(|previous| previous != &requested)
    {
        bail!(
            "--repository: the {kind} surface of {product} was started as {}; run that request \
             again to finish it, or remove what it made before asking for {repository}",
            previous["repository"]
        );
    }
    if let Some(previous) = journal
        .get(&format!("authority/{id}"))?
        .filter(|previous| previous != authority)
    {
        bail!(
            "creation authority changed: request {id} was started against {previous}; resume \
             must use the original catalog and workspace"
        );
    }
    Ok(())
}

pub fn run(args: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(args);
    let [product] = args.positional.as_slice() else {
        bail!("surface add requires one PRODUCT");
    };
    let plan = args.has("--plan");
    if !plan && !args.has("--allow-create") {
        bail!(
            "authority_required: --allow-create is required to provision a repository; \
             `--plan` shows what would be created without it"
        );
    }
    let word = args.required("--kind")?;
    let kind = Surface::parse(word).map_err(|error| anyhow::anyhow!("--kind: {error}"))?;
    let repository = args.required("--repository")?;
    protocol::repository("--repository", repository)?;
    let id = format!("surface-{product}-{kind}");
    protocol::identifier("PRODUCT", &id)?;
    // Every check reads the journal without its writer lock; it is opened
    // for writing only once a repository is to be provisioned, so a plan, a
    // refusal and a surface already declared write nothing.
    let reader = Journal::read(runtime)?;
    let journal = reader.as_ref();
    let requested = json!({"product": product, "kind": kind, "repository": repository});
    let authority = authority(runtime)?;
    let asked = (product.as_str(), kind, repository);
    if let Some(journal) = journal {
        started(journal, &id, asked, &authority)?;
    }
    let document = catalog::load(&runtime.catalog)?;
    let record = catalog::product(&document, product).map_err(|_| {
        anyhow::anyhow!(
            "PRODUCT: no catalogued product {product} in {}; `stado product create` \
             provisions a new product",
            runtime.catalog.display()
        )
    })?;
    let description = catalog::text(record, "description")?;
    // GitHub's limit for a repository description, which the repository
    // receives from the product.
    if description.len() > 350 {
        bail!(
            "PRODUCT: {product}'s catalog description is {} UTF-8 bytes and a repository \
             description holds at most 350; shorten it with `stado product registry set \
             {product} --description …`",
            description.len()
        );
    }
    let declared = record["surfaces"]
        .as_array()
        .context("surfaces must be a list")?
        .iter()
        .find(|surface| surface["kind"] == kind.to_string());
    if let Some(declared) = declared {
        let held = catalog::text(declared, "repository")?;
        if held == repository {
            // Recorded already: by this request (`provisioned`), or by
            // create or by hand (`declared`). Either way nothing is added.
            let ours = journal
                .map(|journal| journal.get(&format!("surface/{id}")))
                .transpose()?
                .flatten()
                .is_some();
            let state = if ours { "provisioned" } else { "declared" };
            let current = record.clone();
            return if plan {
                Plan::done("surface add", &id, state, current).finish(args.has("--json"))
            } else {
                emit(&json!({
                    "request_id": id, "state": state, "product": current,
                    "detail": format!("product {product} already declares the {kind} surface {repository}; nothing was added"),
                }))?;
                Ok(0)
            };
        }
        bail!(
            "--kind: product {product} already declares the {kind} surface {held}; a product \
             has one surface of each kind, so remove it first with `stado product registry set \
             {product} --remove-surface {kind}` if {repository} replaces it"
        );
    }
    let rows = [Proposed {
        field: "--repository".to_owned(),
        repository,
    }];
    let [github] = protocol::distinct::check(journal, &id, None, &rows, &document)?[..] else {
        bail!("the repository check answered for another number of repositories");
    };
    let (checkout, claimed) = super::checkout::target(journal, &id, repository, &runtime.workspace)
        .map_err(|error| anyhow::anyhow!("--repository: {error:#}"))?;
    if plan {
        let record = registry::preview("set", registration(product, kind, repository)?, runtime)?;
        return Plan {
            operation: "surface add",
            request_id: id,
            state: "planned",
            product: json!({"id": product, "name": catalog::text(&record, "name")?}),
            repositories: vec![Row {
                surface: kind,
                repository: repository.to_owned(),
                private: true,
                description: description.to_owned(),
                github,
                checkout,
                checkout_state: if claimed { "claimed" } else { "new" },
            }],
            registration: Some(Registration {
                catalog: runtime.catalog.clone(),
                action: "set",
                record,
            }),
            integration: Some(Integration::read()),
        }
        .finish(args.has("--json"));
    }
    drop(reader);
    let writer = Journal::open(runtime)?;
    let journal = &writer;
    started(journal, &id, asked, &authority)?;
    journal.put(&format!("surface/{id}"), &requested)?;
    journal.put(&format!("authority/{id}"), &authority)?;
    let (observed, checkout) =
        provision(journal, &id, repository, description, &runtime.workspace)?;
    let registration = registry::mutate("set", registration(product, kind, repository)?, runtime)?;
    let observed_catalog = catalog::load(&runtime.catalog)?;
    let record = catalog::product(&observed_catalog, product)?;
    if !record["surfaces"].as_array().is_some_and(|surfaces| {
        surfaces.iter().any(|surface| {
            surface["kind"] == kind.to_string() && surface["repository"] == repository
        })
    }) {
        bail!("product registry read-back does not hold the {kind} surface {repository}");
    }
    let result: Value = json!({
        "request_id": id, "state": "provisioned", "repository": observed, "checkout": checkout,
        "registration": registration, "product": record,
        "detail": "The repository and its catalog surface exist. Implementation, qualification, release and installation have not been established.",
    });
    emit(&result)?;
    Ok(0)
}
