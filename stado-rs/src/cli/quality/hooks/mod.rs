//! `stado quality hooks install|status|remove`: the pre-push gate of every
//! checkout of a product on this machine.
//!
//! Every repository the catalog names for a product (its owner repository
//! and the repository of each surface and installation) has one canonical
//! checkout in the workspace, and every session working in it pushes from
//! there, so one hook in that checkout gates every push made from it. The
//! hook ([`checkout`]) hands Git's ref updates to `stado quality check
//! --pre-push`: a push to `main` that does not pass the commit's declared
//! `format` and `lint` gates is refused before it reaches `main`, and the
//! submit queue's own push passes on the landing its record names.
//! `install` writes or repairs the hook and is idempotent, `status` reads
//! where each checkout stands, `remove` takes the hook out and puts back the
//! one it chained.

mod checkout;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::CmdError;

/// What a verb does in each checkout.
#[derive(Clone, Copy)]
pub enum Verb {
    Install,
    Status,
    Remove,
}

impl Verb {
    fn word(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Status => "status",
            Self::Remove => "remove",
        }
    }
}

/// One repository's checkout and where its pre-push gate stands.
#[derive(Serialize)]
struct Row {
    repository: String,
    /// The catalogued products naming the repository.
    products: Vec<String>,
    /// Its canonical checkout on this machine; none when the workspace holds
    /// none.
    checkout: Option<PathBuf>,
    /// The directory Git runs the checkout's hooks from.
    hooks: Option<PathBuf>,
    /// `installed`, `outdated`, `absent`, `foreign`, `governed`,
    /// `no-checkout` or `unreadable`, after the verb.
    state: String,
    /// Whether a hook the checkout had before is kept and run after Stado's.
    chained: bool,
    /// What the verb did, or why it could not.
    said: String,
    /// Whether the verb failed in this checkout.
    failed: bool,
}

/// Every repository the catalog names for `product`, or for every product,
/// with the products naming each.
fn repositories(
    runtime: &stado_product::common::Runtime,
    product: Option<&str>,
) -> Result<BTreeMap<String, BTreeSet<String>>, CmdError> {
    let catalog = stado_product::catalog::current(runtime).map_err(|error| {
        CmdError::declaration(format!("the product catalog cannot be read: {error:#}"))
    })?;
    let records = catalog["products"].as_array().ok_or_else(|| {
        CmdError::declaration("the product catalog holds no products array".to_string())
    })?;
    let selected: Vec<&serde_json::Value> = match product {
        Some(id) => vec![stado_product::catalog::product(&catalog, id).map_err(|_| {
            let ids: Vec<&str> = records
                .iter()
                .filter_map(|record| record["id"].as_str())
                .collect();
            CmdError::missing(format!(
                "the catalog names no product {id}; it names {}",
                ids.join(", ")
            ))
        })?],
        None => records.iter().collect(),
    };
    let mut repositories: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for record in selected {
        let id = record["id"].as_str().unwrap_or_default().to_string();
        let named = std::iter::once(&record["owner_repository"]).chain(
            ["surfaces", "installations"]
                .iter()
                .flat_map(|list| record[*list].as_array().into_iter().flatten())
                .map(|entry| &entry["repository"]),
        );
        for repository in named.filter_map(serde_json::Value::as_str) {
            repositories
                .entry(repository.to_ascii_lowercase())
                .or_default()
                .insert(id.clone());
        }
    }
    Ok(repositories)
}

/// One repository's row after `verb`.
fn row(
    runtime: &stado_product::common::Runtime,
    repository: String,
    products: BTreeSet<String>,
    verb: Verb,
) -> Row {
    let mut row = Row {
        repository,
        products: products.into_iter().collect(),
        checkout: None,
        hooks: None,
        state: String::new(),
        chained: false,
        said: String::new(),
        failed: false,
    };
    let path = match stado_product::canonical_checkout(runtime, &row.repository) {
        Ok(path) => path,
        Err(error) if error.is::<stado_product::MissingCheckout>() => {
            row.state = "no-checkout".to_string();
            row.said = format!("{error:#}; there is no push from this machine to gate");
            return row;
        }
        Err(error) => {
            row.state = "unreadable".to_string();
            row.said = format!(
                "the checkout of {} cannot be read: {error:#}",
                row.repository
            );
            row.failed = true;
            return row;
        }
    };
    row.checkout = Some(path.clone());
    let inspected = match checkout::inspect(&path) {
        Ok(inspected) => inspected,
        Err(why) => {
            row.state = "unreadable".to_string();
            row.said = why;
            row.failed = true;
            return row;
        }
    };
    let done = match verb {
        Verb::Status => Ok(match inspected.kind() {
            checkout::Kind::Governed => inspected.elsewhere(),
            checkout::Kind::Current => "Stado's pre-push gate runs on every push".to_string(),
            checkout::Kind::Outdated => {
                "another Stado build wrote this hook; `stado quality hooks install` repairs it"
                    .to_string()
            }
            checkout::Kind::Absent => {
                "no pre-push hook: a push runs no gate; `stado quality hooks install` writes it"
                    .to_string()
            }
            checkout::Kind::Foreign => format!(
                "a pre-push hook that is not Stado's: `stado quality hooks install` keeps it as \
                 {} and runs it after the gate",
                checkout::CHAINED
            ),
        }),
        Verb::Install => checkout::install(&inspected),
        Verb::Remove => checkout::remove(&inspected),
    };
    // What the checkout holds after the verb, or as it was when the verb
    // could not change it.
    let after = match &done {
        Ok(_) => checkout::inspect(&path),
        Err(_) => Ok(inspected),
    };
    match (done, after) {
        (Ok(said), Ok(after)) => {
            row.state = after.kind().word().to_string();
            row.hooks = after.runs;
            row.chained = after.chained;
            row.said = said;
        }
        (Err(why), Ok(after)) => {
            row.state = after.kind().word().to_string();
            row.hooks = after.runs;
            row.chained = after.chained;
            row.said = why;
            row.failed = true;
        }
        (_, Err(why)) => {
            row.state = "unreadable".to_string();
            row.said = why;
            row.failed = true;
        }
    }
    row
}

/// Run `verb` in every checkout of `product`, or of every catalogued
/// product with `product` none. Exits non-zero, after every checkout was
/// taken, when one could not be read or written.
pub fn hooks(verb: Verb, product: Option<&str>, json: bool) -> Result<(), CmdError> {
    taken(verb, product, json).map_err(|error| error.machine_readable(json))
}

fn taken(verb: Verb, product: Option<&str>, json: bool) -> Result<(), CmdError> {
    if matches!(verb, Verb::Install) {
        checkout::installed_stado()?;
    }
    let runtime = stado_product::common::Runtime::new(None).map_err(|error| {
        CmdError::declaration(format!("the product workspace cannot be read: {error:#}"))
    })?;
    let rows: Vec<Row> = repositories(&runtime, product)?
        .into_iter()
        .map(|(repository, products)| row(&runtime, repository, products, verb))
        .collect();
    let failed: Vec<String> = rows
        .iter()
        .filter(|row| row.failed)
        .map(|row| format!("{}: {}", row.repository, row.said))
        .collect();
    let document = serde_json::json!({ "verb": verb.word(), "checkouts": &rows });
    if json && failed.is_empty() {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&document)
        );
    }
    if !json {
        for row in &rows {
            let place = row.checkout.as_ref().map_or_else(
                || "no checkout".to_string(),
                |path| path.display().to_string(),
            );
            // A kept hook runs after Stado's only where Git runs Stado's.
            let chained = row.chained
                && matches!(row.state.as_str(), "installed" | "outdated")
                && !row.said.contains(checkout::CHAINED);
            let note = if chained {
                format!("; its earlier hook runs after it as {}", checkout::CHAINED)
            } else {
                String::new()
            };
            println!(
                "{} ({}) {}: {}{note}",
                row.repository, place, row.state, row.said
            );
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "stado quality hooks {}: {} of {} checkout(s) could not be taken: {}",
        verb.word(),
        failed.len(),
        rows.len(),
        failed.join("; ")
    ))
    .stating(crate::primitives::failure::FailureCode::Config)
    .detailing(document))
}
