//! Whether a declared service answers on the path it is for, and what the
//! host does when it does not.
//!
//! A service may accept connections without answering its declared API path.
//! The catalog's `service.answering` names the method, path and body to ask
//! locally on each beacon pass. A request runs to completion in its own task;
//! its start is recorded. The next pass judges an unanswered request against
//! the host's declared directory refresh interval, the same bound its
//! resolver applies to held requests. [`recover`] runs the declared repair
//! and restart, records the event and alerts; `stado service recoveries`
//! reads the register. The beacon carries each probe's outcome under the
//! unit's `answering`. [`peer`] uses the same recovery path when another
//! host's resolver observes a held service.

pub(crate) mod episode;
pub mod peer;
pub mod recover;
pub mod register;

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::deploy::service_catalog::{AnsweringProbe, CatalogService};
use episode::{Completed, Question};

/// What the newest finished probe of a service found.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Finished {
    /// The service sent an HTTP answer. A failed status keeps the response
    /// body, which names the operation and cause that readiness refused.
    Answered {
        status: u16,
        successful: bool,
        elapsed_ms: u128,
        at: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// The request could not be sent or ended in a transport error: the
    /// service is not answering either, with the transport's own words.
    Refused { cause: String, at: String },
}

/// What this pass reads about a service's answering.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
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
    /// The declared question could not be asked because its catalog,
    /// directory address or standing bound could not be read.
    Unreadable { cause: String },
    /// No question is in flight; this is what the newest one found.
    Finished { result: Finished },
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
    question: Arc<Question>,
    since: String,
    started: Instant,
    /// The task is created from a runtime request after this record exists.
    abort: OnceLock<tokio::task::AbortHandle>,
}

static PENDING: Mutex<BTreeMap<String, Arc<Pending>>> = Mutex::new(BTreeMap::new());
static FINISHED: Mutex<BTreeMap<String, Completed>> = Mutex::new(BTreeMap::new());
static CLIENT: LazyLock<Result<reqwest::Client, reqwest::Error>> =
    LazyLock::new(|| reqwest::Client::builder().build());

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
pub fn ask_and_read(
    entry: &CatalogService,
    unit: &str,
    port: u16,
    bound_seconds: u64,
) -> Option<Reading> {
    let declared = entry.answering.as_ref()?;
    let name = entry.name.as_str();
    let mut active = held(&PENDING);
    // A redeployment can retain the name while changing the endpoint.
    let changed = active
        .get(name)
        .is_some_and(|pending| !pending.question.matches(unit, port, declared));
    if changed {
        if let Some(pending) = active.remove(name) {
            if let Some(abort) = pending.abort.get() {
                abort.abort();
            }
        }
    }
    let mut finished = held(&FINISHED);
    if changed
        || finished
            .get(name)
            .is_some_and(|answer| !answer.question.matches(unit, port, declared))
    {
        finished.remove(name);
        episode::forget(name);
    }
    drop(finished);
    if !active.contains_key(name) {
        let question = Arc::new(Question::new(unit, port, declared));
        let request = CLIENT
            .as_ref()
            .map_err(|error| format!("the service answering HTTP client cannot start: {error}"))
            .and_then(|client| build(client, declared, port));
        match request {
            Err(cause) => {
                held(&FINISHED).insert(
                    name.to_owned(),
                    Completed {
                        question,
                        result: Finished::Refused {
                            cause,
                            at: now_iso(),
                        },
                    },
                );
            }
            Ok(request) => {
                let pending = Arc::new(Pending {
                    question,
                    since: now_iso(),
                    started: Instant::now(),
                    abort: OnceLock::new(),
                });
                active.insert(name.to_owned(), Arc::clone(&pending));
                let service = name.to_owned();
                let running = Arc::clone(&pending);
                let task = tokio::spawn(async move {
                    let started = Instant::now();
                    let found = match crate::wait::request(request).await {
                        Ok(response) => {
                            let status = response.status();
                            let detail = if status.is_success() {
                                None
                            } else {
                                Some(match response.text().await {
                                    Ok(body) => body,
                                    Err(error) => format!(
                                        "HTTP {status} response body could not be read: {error}"
                                    ),
                                })
                            };
                            Finished::Answered {
                                status: status.as_u16(),
                                successful: status.is_success(),
                                elapsed_ms: started.elapsed().as_millis(),
                                at: now_iso(),
                                detail,
                            }
                        }
                        Err(error) => Finished::Refused {
                            cause: error.to_string(),
                            at: now_iso(),
                        },
                    };
                    let mut active = held(&PENDING);
                    if active
                        .get(&service)
                        .is_some_and(|current| Arc::ptr_eq(current, &running))
                    {
                        active.remove(&service);
                        held(&FINISHED).insert(
                            service,
                            Completed {
                                question: Arc::clone(&running.question),
                                result: found,
                            },
                        );
                    }
                });
                pending.abort.get_or_init(|| task.abort_handle());
            }
        }
    }
    drop(active);
    Some(read(name, bound_seconds))
}

/// The standing of `service` as the records say it now.
pub fn read(service: &str, bound_seconds: u64) -> Reading {
    let active = held(&PENDING);
    let last = held(&FINISHED)
        .get(service)
        .map(|answer| answer.result.clone());
    match active.get(service) {
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
            Some(found) => Reading::Finished { result: found },
            None => Reading::Asked { since: now_iso() },
        },
    }
}

/// Forget both the question and its last answer after a recovery attempt.
/// Cancel the in-flight request so repeated attempts do not retain abandoned
/// tasks and connections. A late answer cannot settle the next question.
pub fn forget(service: &str) {
    let mut active = held(&PENDING);
    if let Some(pending) = active.remove(service) {
        if let Some(abort) = pending.abort.get() {
            abort.abort();
        }
    }
    held(&FINISHED).remove(service);
}

/// A completed declaration pass cancels requests not asked this pass and
/// discards their last answers. An unreadable endpoint retains its declared
/// recovery episode, avoiding another restart when the directory recovers;
/// removing the question retires that episode too.
pub(crate) fn retain_declared(questions: &HashMap<String, bool>) {
    let mut active = held(&PENDING);
    active.retain(|name, pending| {
        if questions.get(name).copied() == Some(true) {
            return true;
        }
        if let Some(abort) = pending.abort.get() {
            abort.abort();
        }
        false
    });
    held(&FINISHED).retain(|name, _| questions.get(name).copied() == Some(true));
    episode::retain_declared(questions);
}
