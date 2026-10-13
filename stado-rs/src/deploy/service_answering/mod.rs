//! Whether a declared service answers on the path it is for, and what the
//! host does when it does not.
//!
//! A service that accepts every connection and answers none passes a
//! readiness read and fails everyone: its `/health` answered while the path
//! the service exists for held every request for hours, the resolver on
//! each client recorded the waits, and nothing on the host that ran the
//! service acted, because nothing on it asked the question the clients
//! were asking. The catalog's `service.answering` declares that question —
//! the method, path and body of one request on the service's own API — and
//! the host process asks it on every beacon pass, at the service's listen
//! port on this host. The request runs to its end in a task of its own and
//! its start is recorded, the way the resolver records every wait; the
//! next pass reads the record and judges a question unanswered longer than
//! this host's declared directory refresh interval — the one measure of
//! standing the resolver already applies to a held adapter — as a service
//! that stands. [`recover`] then runs its declared repair and restart,
//! records the event in the host's recovery register and alerts; `stado
//! service recoveries` reads the register. The beacon carries each probe's
//! outcome under the unit's `answering`.

pub mod recover;
pub mod register;

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Value};

use crate::deploy::service_catalog::{AnsweringProbe, CatalogService};

/// What the newest finished probe of a service found.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Finished {
    /// The service sent an answer.
    Answered {
        status: u16,
        elapsed_ms: u128,
        at: String,
    },
    /// The request could not be sent or ended in a transport error: the
    /// service is not answering either, with the transport's own words.
    Refused { cause: String, at: String },
}

/// What this pass reads about a service's answering.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Reading {
    /// The first question was just sent; nothing is known yet.
    Asked { since: String },
    /// A question is in flight and younger than the bound; the newest
    /// finished probe, when there is one, is carried beside it.
    Waiting {
        since: String,
        waited_seconds: u64,
        last: Option<Finished>,
    },
    /// A question has waited longer than the bound: the service stands.
    Stood {
        since: String,
        waited_seconds: u64,
        bound_seconds: u64,
    },
    /// No question is in flight; this is what the newest one found.
    Finished(Finished),
}

impl Reading {
    pub fn stands(&self) -> bool {
        matches!(self, Reading::Stood { .. })
    }

    pub fn document(&self) -> Value {
        json!(self)
    }
}

struct Pending {
    since: String,
    started: Instant,
}

static PENDING: Mutex<BTreeMap<String, Pending>> = Mutex::new(BTreeMap::new());
static FINISHED: Mutex<BTreeMap<String, Finished>> = Mutex::new(BTreeMap::new());

pub(super) fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The records, held even after a task that held them panicked: a probe's
/// record is a plain map, and a poisoned lock on it says nothing about the
/// map's content.
fn held<T>(lock: &'static Mutex<T>) -> MutexGuard<'static, T> {
    match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The request the declaration describes, built against `port` on this
/// host: every declared service listens on the loopback of the host it
/// runs on, which is where its directory endpoint points.
fn build(
    client: &reqwest::Client,
    probe: &AnsweringProbe,
    port: u16,
) -> Result<reqwest::RequestBuilder, String> {
    let method = reqwest::Method::from_bytes(probe.method.as_bytes()).map_err(|error| {
        format!(
            "the declared answering method {} is not an HTTP method: {error}",
            probe.method
        )
    })?;
    let mut request = client.request(
        method,
        format!("http://{}:{port}{}", Ipv4Addr::LOCALHOST, probe.path),
    );
    if let Some(body) = &probe.body {
        request = request.json(body);
    }
    Ok(request)
}

/// Ask `entry`'s declared question at `port` unless one is already in
/// flight: the request runs to its end in a task of its own and records
/// what it found. Then read the service's standing against `bound_seconds`.
/// `None` for a service that declares no question.
pub fn ask_and_read(entry: &CatalogService, port: u16, bound_seconds: u64) -> Option<Reading> {
    let declared = entry.answering.as_ref()?;
    let name = entry.name.clone();
    let in_flight = held(&PENDING).contains_key(&name);
    if !in_flight {
        let client = reqwest::Client::new();
        match build(&client, declared, port) {
            Err(cause) => {
                held(&FINISHED).insert(
                    name.clone(),
                    Finished::Refused {
                        cause,
                        at: now_iso(),
                    },
                );
            }
            Ok(request) => {
                held(&PENDING).insert(
                    name.clone(),
                    Pending {
                        since: now_iso(),
                        started: Instant::now(),
                    },
                );
                let service = name.clone();
                tokio::spawn(async move {
                    let started = Instant::now();
                    let found = match crate::wait::request(request).await {
                        Ok(response) => Finished::Answered {
                            status: response.status().as_u16(),
                            elapsed_ms: started.elapsed().as_millis(),
                            at: now_iso(),
                        },
                        Err(error) => Finished::Refused {
                            cause: error.to_string(),
                            at: now_iso(),
                        },
                    };
                    held(&PENDING).remove(&service);
                    held(&FINISHED).insert(service, found);
                });
            }
        }
    }
    Some(read(&name, bound_seconds))
}

/// The standing of `service` as the records say it now.
pub fn read(service: &str, bound_seconds: u64) -> Reading {
    let last = held(&FINISHED).get(service).cloned();
    match held(&PENDING).get(service) {
        Some(wait) => {
            let waited_seconds = wait.started.elapsed().as_secs();
            if waited_seconds > bound_seconds {
                Reading::Stood {
                    since: wait.since.clone(),
                    waited_seconds,
                    bound_seconds,
                }
            } else if last.is_none() && waited_seconds < bound_seconds {
                Reading::Asked {
                    since: wait.since.clone(),
                }
            } else {
                Reading::Waiting {
                    since: wait.since.clone(),
                    waited_seconds,
                    last,
                }
            }
        }
        None => match last {
            Some(found) => Reading::Finished(found),
            None => Reading::Asked { since: now_iso() },
        },
    }
}

/// Forget the in-flight question of `service`, after its process was
/// restarted: the request that stood belongs to the process that is gone,
/// and the next pass asks the new one afresh.
pub fn forget(service: &str) {
    held(&PENDING).remove(service);
}
