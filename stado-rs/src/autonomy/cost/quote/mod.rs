//! Allocation quotes use recorded worker ownership and provider inventory, never
//! requested job shapes. A report can contain both quoted and unpriced jobs.
mod placement;
mod schema;

use super::{prices::PriceState, PriceBook, PriceQuote};
use crate::autonomy::{
    model::{InventorySnapshot, SourceState},
    storage,
};
use crate::capabilities::ProviderId;
use crate::machine::{MachineFacade, AGENT_INSTANCE_PREFIX};
use crate::models::{Job, WorkerResource};
use crate::queue::JobStorage;
use chrono::Utc;
use schema::{HourUnit, Purchase};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Serialize)]
pub struct AllocationQuote {
    pub job_id: String,
    pub allocation: Value,
    pub quote: Option<PriceQuote>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct AllocationQuotes {
    pub created_at: String,
    pub complete: bool,
    pub book_created_at: Option<String>,
    pub book_error: Option<String>,
    pub price_sources: Vec<Value>,
    pub inventory_snapshot_id: Option<String>,
    pub inventory_created_at: Option<String>,
    pub inventory_error: Option<String>,
    pub inventory_sources: Vec<Value>,
    pub quotes: Vec<AllocationQuote>,
}

pub async fn quote_jobs(store: &JobStorage, job_ids: &[String]) -> AllocationQuotes {
    let created_at = Utc::now().to_rfc3339();
    let (book, book_error) =
        match storage::read_json::<PriceBook>(store, "state/autonomy/cost/prices.json").await {
            Ok(Some(book)) => (Some(book), None),
            Ok(None) => (
                None,
                Some(
                    "provider price book absent; Stado optimize publishes price observations"
                        .into(),
                ),
            ),
            Err(error) => (
                None,
                Some(format!("read state/autonomy/cost/prices.json: {error}")),
            ),
        };
    let (inventory, inventory_error) = match storage::load_latest_inventory(store).await {
        Ok(Some(inventory)) => (Some(inventory), None),
        Ok(None) => (
            None,
            Some(
                "provider inventory snapshot absent; Stado inventory publishes observations".into(),
            ),
        ),
        Err(error) => (
            None,
            Some(format!("read latest provider inventory: {error}")),
        ),
    };
    let facade = MachineFacade::with_store(store.clone(), crate::config::bucket());
    let mut quotes = Vec::with_capacity(job_ids.len());
    for job_id in job_ids {
        let mut row = AllocationQuote {
            job_id: job_id.clone(),
            allocation: Value::Null,
            quote: None,
            error: None,
        };
        match facade.lookup_job(job_id).await {
            Ok(job) => {
                row.job_id.clone_from(&job.job_id);
                row.allocation = json!({"job": facade.observed_job(&job).await});
                match select(&job, book.as_ref(), inventory.as_ref(), &mut row.allocation) {
                    Ok(quote) => row.quote = Some(quote.clone()),
                    Err(error) => row.error = Some(error),
                }
            }
            Err(error) => row.error = Some(format!("read job {job_id}: {error}")),
        }
        quotes.push(row);
    }
    AllocationQuotes {
        created_at,
        complete: quotes.iter().all(|row| row.quote.is_some() && row.error.is_none()),
        book_created_at: book.as_ref().map(|book| book.created_at.clone()),
        book_error,
        price_sources: book.as_ref().map(|book| book.sources.iter().map(|source| json!({
            "provider": source.provider, "state": source.state, "source": source.source,
            "observed_at": source.observed_at, "error": source.error,
        })).collect()).unwrap_or_default(),
        inventory_snapshot_id: inventory.as_ref().map(|snapshot| snapshot.snapshot_id.clone()),
        inventory_created_at: inventory.as_ref().map(|snapshot| snapshot.created_at.clone()),
        inventory_error,
        inventory_sources: inventory.as_ref().map(|snapshot| snapshot.sources.iter().map(|source| json!({
            "provider": source.provider, "account": source.account, "state": source.state,
            "observed_at": source.observed_at, "coverage": source.coverage, "upstream_error": source.upstream_error,
        })).collect()).unwrap_or_default(),
        quotes,
    }
}

fn source_complete(book: &PriceBook, provider: ProviderId) -> Result<(), String> {
    let mut observed = false;
    for source in book
        .sources
        .iter()
        .filter(|source| source.provider == provider)
    {
        observed = true;
        if source.state != PriceState::Complete {
            return Err(format!("price source {:?} is {:?}: {:?}; incomplete observations cannot prove a unique quote", provider, source.state, source.error));
        }
    }
    if observed {
        Ok(())
    } else {
        Err(format!(
            "price book has no source observation for {provider:?}"
        ))
    }
}

fn unique<'a>(
    mut candidates: impl Iterator<Item = &'a PriceQuote>,
    context: &str,
) -> Result<&'a PriceQuote, String> {
    let selected = candidates.next().ok_or_else(|| format!("no complete whole-machine USD hourly quote for {context}; component prices are not a machine shape"))?;
    if candidates.any(|candidate| candidate != selected) {
        return Err(format!(
            "multiple distinct whole-machine quotes match {context}; no cheapest quote was chosen"
        ));
    }
    Ok(selected)
}

fn select<'a>(
    job: &Job,
    book: Option<&'a PriceBook>,
    inventory: Option<&InventorySnapshot>,
    allocation: &mut Value,
) -> Result<&'a PriceQuote, String> {
    let reference = job.instance_ref.as_deref().filter(|reference| !reference.is_empty())
        .ok_or("job has no recorded allocation; purchase and machine requests are not placement evidence")?;
    let worker = job.worker_allocation.as_ref().ok_or(
        "job has no worker-origin observation; an agent reference is not a physical provider",
    )?;
    if reference.strip_prefix(AGENT_INSTANCE_PREFIX) != Some(worker.host.as_str()) {
        return Err("worker-origin observation does not belong to this execution's agent".into());
    }
    if let Some(error) = &worker.error {
        return Err(format!("worker identity observation: {error}"));
    }
    let identity = worker
        .resource
        .as_ref()
        .ok_or("worker omitted its resource identity")?;
    allocation["worker"] = json!(worker);
    if matches!(identity, WorkerResource::Local) {
        let book = book.ok_or("provider price book unavailable; see book_error")?;
        source_complete(book, ProviderId::Local)?;
        return unique(
            book.quotes.iter().filter(|quote| {
                quote.provider == ProviderId::Local
                    && quote.source == "autonomy policy"
                    && quote.machine_type.is_none()
                    && Purchase::decode(&quote.purchase_option).ok() == Some(Purchase::OnDemand)
                    && HourUnit::accepts(&quote.unit)
                    && quote.currency == "USD"
                    && quote.hourly_usd.is_finite()
                    && quote.hourly_usd >= 0.0
            }),
            reference,
        );
    }
    let inventory = inventory.ok_or("provider inventory unavailable; see inventory_error")?;
    let resource = placement::resource(job, inventory)?;
    allocation["resource"] = json!(resource);
    let mut observed = false;
    for source in inventory
        .sources
        .iter()
        .filter(|source| source.provider == resource.provider)
    {
        observed = true;
        if source.state != SourceState::Complete {
            return Err(format!(
                "inventory source {:?} account {} is {:?}: {:?}",
                source.provider, source.account, source.state, source.upstream_error
            ));
        }
    }
    if !observed {
        return Err("inventory omitted its allocation provider's source observation".into());
    }
    let (machine, purchase) = placement::attributes(resource)?;
    let book = book.ok_or("provider price book unavailable; see book_error")?;
    source_complete(book, resource.provider)?;
    let selected = unique(
        book.quotes
            .iter()
            .filter(|quote| placement::matches(quote, resource, machine, purchase)),
        &resource.resource_id,
    )?;
    let started = placement::timestamp(
        job.started_at
            .as_deref()
            .ok_or("allocated job omitted started_at")?,
        "job start",
    )?;
    if placement::timestamp(&selected.observed_at, "price observation")? < started {
        return Err(format!(
            "price observation {} predates this allocation; no freshness interval was invented",
            selected.observed_at
        ));
    }
    Ok(selected)
}
