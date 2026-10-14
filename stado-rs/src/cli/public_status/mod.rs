//! `stado public-status check | show | serve` — whether the components people
//! rely on work, as the fleet sees them from outside.
//!
//! Each catalogued product declares, under `public_status`, the components a
//! person uses and the one request that shows each works (front page, sign-in,
//! an API answering its route). `check` sends every declared request once,
//! from the machine it runs on, and publishes the outcome beside what the last
//! check recorded (`public-status/components.json` in the object store):
//! each component's state, since when, and the last failed check with the
//! answer that failed it. `show` reads that document, and `serve` is the
//! public status page that reads it on every request, run as its own unit on
//! a host apart from the products it reports on. Whatever schedule runs
//! `check` (`stado schedule create --cron … 'stado public-status check'`) is
//! its cadence. Nothing here keeps a list of components: the catalog does.

mod checks;
mod document;
mod page;
mod serve;

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

use crate::cli::entry::spec::root::installation::PublicStatusCommands;
use crate::cli::CmdError;
use crate::queue::JobStorage;

/// `stado serve --public-status-bind HOST:PORT`: the page as a role of the
/// host's one Stado process.
pub(crate) async fn serve_role(bind: String) -> Result<(), CmdError> {
    serve::serve(&bind).await
}

pub(crate) async fn dispatch(command: PublicStatusCommands) -> Result<(), CmdError> {
    if let PublicStatusCommands::Serve { bind } = &command {
        return serve::serve(bind).await;
    }
    let store = JobStorage::with_bucket(crate::config::bucket()).await?;
    match command {
        PublicStatusCommands::Check { json } => {
            let declared = checks::declared()?;
            let previous = document::read(&store).await?;
            let checked_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
            let mut observed = Vec::new();
            for item in &declared {
                let looked = checks::look(&item.check).await;
                observed.push(document::Observed {
                    product: item.product.clone(),
                    component: item.check.component.clone(),
                    title: item.check.title.clone(),
                    up: looked.up,
                    detail: looked.detail,
                });
            }
            let checked_by = crate::providers::vast::system_hostname();
            let published = document::fold(previous.as_ref(), &observed, &checked_at, &checked_by);
            document::publish(&store, &published).await?;
            emit(&published, json)
        }
        PublicStatusCommands::Show { json } => match document::read(&store).await? {
            Some(published) => emit(&published, json),
            None => Err(CmdError::refused(format!(
                "no status is published at {} in the object store: no `stado public-status \
                 check` has run against it",
                document::BLOB
            ))),
        },
        PublicStatusCommands::Serve { bind } => serve::serve(&bind).await,
    }
}

/// The document as JSON, or one line per component for a person.
fn emit(document: &Value, json: bool) -> Result<(), CmdError> {
    if json {
        println!("{}", serde_json::to_string_pretty(document)?);
        return Ok(());
    }
    println!(
        "checked at {} by {}",
        text(&document["checked_at"]),
        text(&document["checked_by"])
    );
    for row in document["components"].as_array().into_iter().flatten() {
        let failure = match row["last_failure"].as_object() {
            Some(failure) => format!(
                "; last failed at {}: {}",
                text(&failure["at"]),
                text(&failure["error"])
            ),
            None => "; no failed check recorded".to_owned(),
        };
        println!(
            "{} ({}/{}): {} since {}{failure}",
            text(&row["title"]),
            text(&row["product"]),
            text(&row["component"]),
            text(&row["state"]),
            text(&row["since"])
        );
    }
    Ok(())
}

/// A field as it reads in a line: a string without quotes, anything else as
/// JSON.
fn text(value: &Value) -> String {
    match value.as_str() {
        Some(text) => text.to_owned(),
        None => value.to_string(),
    }
}
