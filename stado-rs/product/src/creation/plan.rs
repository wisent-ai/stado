//! `--plan`: everything `stado product create` or `stado product surface add`
//! checks before it provisions anything, carried out with reads only, and
//! what it would then create. The journal is read without its writer lock,
//! the catalog record is built and validated in memory, GitHub is asked with
//! `gh api` (a read), and the integration grant is read but not presented.
//! No repository, checkout, journal entry or catalog line is written.

use super::{
    authority, checkout, existing, marker, proposed, protocol, provider, registration,
    state::Journal, Request, Surface,
};
use crate::{
    catalog,
    common::{emit, Runtime},
    registry,
};
use anyhow::{bail, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// One repository a plan would provision.
#[derive(Serialize)]
pub struct Row {
    pub surface: Surface,
    pub repository: String,
    pub private: bool,
    pub description: String,
    /// `free` (GitHub serves no such repository), `own` (this request
    /// provisioned it already) or `registered` (the catalog holds it from
    /// this request).
    pub github: &'static str,
    pub checkout: PathBuf,
    /// `new` or `claimed` (this request made it already).
    pub checkout_state: &'static str,
}

/// The catalog change a plan would make.
#[derive(Serialize)]
pub struct Registration {
    pub catalog: PathBuf,
    /// `add` (a new product), `set` (a surface on an existing one) or
    /// `registered` (this request registered it already).
    pub action: &'static str,
    pub record: Value,
}

/// What provisioning would present to Wisent Integrations: its origin, or
/// why provisioning would stop before the first repository.
#[derive(Serialize)]
pub struct Integration {
    pub origin: Option<String>,
    pub blocker: Option<String>,
}

impl Integration {
    pub fn read() -> Self {
        match provider::integration() {
            Ok(integration) => Self {
                origin: Some(integration.origin),
                blocker: None,
            },
            Err(error) => Self {
                origin: None,
                blocker: Some(format!("{error:#}")),
            },
        }
    }
}

#[derive(Serialize)]
pub struct Plan {
    /// `create` or `surface add`.
    pub operation: &'static str,
    pub request_id: String,
    /// `planned`; `provisioned` when the request has nothing left to do;
    /// `declared` when the catalog already holds the surface asked for.
    pub state: &'static str,
    pub product: Value,
    pub repositories: Vec<Row>,
    pub registration: Option<Registration>,
    pub integration: Option<Integration>,
}

impl Plan {
    /// Nothing would be created: the request was carried out to the end
    /// (`provisioned`) or the catalog already holds what it asks for
    /// (`declared`).
    pub fn done(
        operation: &'static str,
        request_id: &str,
        state: &'static str,
        product: Value,
    ) -> Self {
        Self {
            operation,
            request_id: request_id.to_owned(),
            state,
            product,
            repositories: Vec::new(),
            registration: None,
            integration: None,
        }
    }

    /// Print the plan as JSON or text; exit 1 when it names a blocker.
    pub fn finish(&self, json_output: bool) -> Result<i32> {
        if json_output {
            emit(&serde_json::to_value(self)?)?;
        } else {
            print!("{}", self.text()?);
        }
        match self
            .integration
            .as_ref()
            .and_then(|integration| integration.blocker.as_ref())
        {
            Some(blocker) => {
                eprintln!(
                    "Error: {} would stop before provisioning: {blocker}",
                    self.operation
                );
                Ok(1)
            }
            None => Ok(0),
        }
    }

    fn text(&self) -> Result<String> {
        let (operation, id) = (self.operation, &self.request_id);
        match self.state {
            "provisioned" => {
                return Ok(format!(
                    "Plan for {operation} request {id}: it is already provisioned, so \
                     {operation} would create nothing. Nothing was created.\n"
                ))
            }
            "declared" => {
                return Ok(format!(
                    "Plan for {operation} request {id}: the catalog already declares this \
                     surface, so {operation} would create nothing. Nothing was created.\n"
                ))
            }
            _ => {}
        }
        let mut out = format!("Plan for {operation} request {id}. Nothing was created.\n");
        let product = catalog::text(&self.product, "id")?;
        if operation == "create" {
            out.push_str(&format!(
                "Product {product} ({}), family {}, visibility {}, catalog status preview.\n",
                catalog::text(&self.product, "name")?,
                catalog::text(&self.product, "family")?,
                catalog::text(&self.product, "visibility")?,
            ));
        } else {
            out.push_str(&format!("Existing product {product}.\n"));
        }
        for row in &self.repositories {
            let github = match row.github {
                "free" => "free on GitHub, would be created private",
                "own" => "provisioned by this request already",
                _ => "registered by this request already",
            };
            let checkout = match row.checkout_state {
                "claimed" => "checkout made by this request already",
                _ => "checkout would be created",
            };
            out.push_str(&format!(
                "  {:<8} {}  {github}; {checkout} at {}\n",
                row.surface.to_string(),
                row.repository,
                row.checkout.display(),
            ));
        }
        if let Some(row) = self.repositories.first() {
            out.push_str(&format!("Repository description: {}\n", row.description));
        }
        if let Some(registration) = &self.registration {
            let catalog = registration.catalog.display();
            let record = &registration.record;
            let mut surfaces = Vec::new();
            for surface in record["surfaces"].as_array().into_iter().flatten() {
                surfaces.push(format!(
                    "{}={}",
                    catalog::text(surface, "kind")?,
                    catalog::text(surface, "repository")?
                ));
            }
            let surfaces = surfaces.join(", ");
            out.push_str(&match registration.action {
                "add" => format!(
                    "Catalog {catalog}: would register {product} (owner repository {}) with \
                     surfaces {surfaces}.\n",
                    catalog::text(record, "owner_repository")?
                ),
                "set" => {
                    format!("Catalog {catalog}: would record {product}'s surfaces as {surfaces}.\n")
                }
                _ => {
                    format!("Catalog {catalog}: {product} is registered by this request already.\n")
                }
            });
        }
        if let Some(origin) = self
            .integration
            .as_ref()
            .and_then(|integration| integration.origin.as_ref())
        {
            out.push_str(&format!(
                "Wisent Integrations at {origin} would provision the repositories (not \
                 contacted).\n"
            ));
        }
        Ok(out)
    }
}

/// The plan of `request`.
pub fn create(request: &Request, runtime: &Runtime, json_output: bool) -> Result<i32> {
    let id = &request.request_id;
    let journal = Journal::read(runtime)?;
    let authority = authority(runtime)?;
    if let Some(journal) = &journal {
        let value = serde_json::to_value(request)?;
        if journal
            .get(&format!("request/{id}"))?
            .is_some_and(|previous| previous != value)
        {
            bail!(
                "request_id: request_id_conflict: creation request {id} is already journaled \
                 with another payload; `stado product create --status {id}` shows it, and a \
                 changed request needs a new request_id"
            );
        }
        if let Some(previous) = journal
            .get(&format!("authority/{id}"))?
            .filter(|previous| previous != &authority)
        {
            bail!(
                "creation authority changed: request {id} was started against {previous}; \
                 resume must use the original catalog and workspace"
            );
        }
        if journal
            .get(&format!("response/{id}"))?
            .is_some_and(|response| response["state"] == "provisioned")
        {
            return Plan::done(
                "create",
                id,
                "provisioned",
                serde_json::to_value(&request.product)?,
            )
            .finish(json_output);
        }
    }
    let document = catalog::load(&runtime.catalog)?;
    let registered = existing(&document, request)?;
    let rows = proposed(request);
    let answers = match registered {
        Some(_) => vec!["registered"; rows.len()],
        None => protocol::distinct::check(
            journal.as_ref(),
            id,
            Some(("product.id", &request.product.id)),
            &rows,
            &document,
        )?,
    };
    let mut repositories = Vec::with_capacity(rows.len());
    for ((row, github), proposed) in request.repositories.iter().zip(answers).zip(&rows) {
        let (checkout, claimed) =
            checkout::target(journal.as_ref(), id, &row.repository, &runtime.workspace)
                .map_err(|error| anyhow::anyhow!("{}: {error:#}", proposed.field))?;
        repositories.push(Row {
            surface: row.surface,
            repository: row.repository.clone(),
            private: true,
            description: request.product.description.clone(),
            github,
            checkout,
            checkout_state: if claimed { "claimed" } else { "new" },
        });
    }
    let registration = match registered {
        Some(record) => Registration {
            catalog: runtime.catalog.clone(),
            action: "registered",
            record: record.clone(),
        },
        None => Registration {
            catalog: runtime.catalog.clone(),
            action: "add",
            record: registry::preview("add", registration(request)?, runtime)?,
        },
    };
    let mut product = serde_json::to_value(&request.product)?;
    product["marker"] = json!(marker(id));
    Plan {
        operation: "create",
        request_id: id.clone(),
        state: "planned",
        product,
        repositories,
        registration: Some(registration),
        integration: Some(Integration::read()),
    }
    .finish(json_output)
}
