use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::service_resolution;
use crate::targets::{self, RegistryStore};

use crate::cli::CmdError;

pub(in crate::cli::resolver) mod document;
pub(in crate::cli::resolver) mod source;
mod unanswered;
pub(in crate::cli::resolver) mod write;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPayload {
    store_version: String,
    document: Value,
}

fn validate_snapshot(payload: SnapshotPayload) -> Result<(Value, String, u64), String> {
    targets::validate_registry(&payload.document).map_err(|error| error.to_string())?;
    let directory = service_resolution::directory(&payload.document)?
        .ok_or_else(|| "registry.service_directory is required".to_string())?;
    Ok((
        payload.document,
        payload.store_version,
        directory.generation,
    ))
}

/// Read the selected registry store without requiring a service directory.
/// Each caller validates the document for the surface that consumes it.
pub(crate) async fn read_local_document(
    store: &RegistryStore,
) -> Result<(Value, String), CmdError> {
    let blob = store
        .read_versioned()
        .await
        .map_err(|error| {
            let mut error = CmdError::from(error);
            if let Some(message) = error.message.as_mut() {
                *message = format!("registry read failed: {message}");
            }
            error
        })?
        .ok_or_else(|| crate::cli::registry::registry_absent(store.location()))?;
    let document: Value = serde_json::from_str(&blob.content).map_err(|error| {
        CmdError::click(format!("invalid registry JSON: {error}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    Ok((document, blob.version))
}

pub(crate) async fn read_local_snapshot(
    store: &RegistryStore,
) -> Result<(Value, String, u64), CmdError> {
    let (document, store_version) = read_local_document(store).await?;
    // Every refusal here is the registry document failing its contract or
    // declaring no service directory: the declaration is what to fix.
    validate_snapshot(SnapshotPayload {
        store_version,
        document,
    })
    .map_err(CmdError::declaration)
}

/// Print the registry generation this host's running resolver holds, as
/// every other host reads the authority over SSH.
///
/// Asked of the resolver's API, never read from the registry store: on the
/// authority that store is the object API, which authorizes each read
/// through Skarbiec, and when Skarbiec stands the snapshot would stand with
/// it, so every registry read of every host would wait on one command.
/// The resolver reads the store on its own interval and holds the result;
/// this command only asks for it, in one registered wait on a loopback
/// address, and is refused at once with the reason when no live resolver
/// publishes one or the one it holds is past its declared max-stale. A
/// resolver that accepts the request and answers nothing is given up at its
/// own refresh interval: the reader over SSH gives up at that interval too,
/// and a snapshot command left waiting after it is a process and a
/// connection the authority keeps for nobody.
pub(super) async fn emit_snapshot() -> Result<(), CmdError> {
    use crate::cli::resolver::report::published::{published_state, publisher_running, state_path};
    use crate::primitives::failure::FailureCode;
    let refused = |detail: String| CmdError::click(detail).stating(FailureCode::InfraDown);
    let Some(published) = published_state() else {
        return Err(refused(match state_path() {
            Some(path) => format!(
                "no resolver has published its state at {}, so no registry generation is held \
                 here to answer from; stado serve runs the resolver",
                path.display()
            ),
            None => "HOME is unset, so the resolver's published state cannot be found".to_string(),
        }));
    };
    if !publisher_running(&published) {
        return Err(refused(format!(
            "the resolver that published its state (pid {}) is not running, so no registry \
             generation is held here to answer from",
            published.pid
        )));
    }
    let Some(bind) = published.api_bind.as_deref() else {
        return Err(refused(format!(
            "the running resolver (pid {}) publishes no API address: it predates answering \
             registry snapshots from memory",
            published.pid
        )));
    };
    let url = format!(
        "http://{bind}{}",
        crate::cli::resolver::serve::SNAPSHOT_PATH
    );
    let route = url::Url::parse(&url).map_err(|error| {
        refused(format!(
            "the resolver's API address {url} is not a URL: {error}"
        ))
    })?;
    let did_not_answer =
        |error: reqwest::Error| refused(format!("the resolver at {url} did not answer: {error}"));
    // A loopback address is this host's own resolver: no proxy a shell's
    // environment names sits between them.
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(did_not_answer)?;
    let sent = crate::wait::send(
        crate::wait::Kind::Network,
        "the resolver's held registry generation",
        client.get(&url),
    );
    let response =
        crate::cli::resolver::unless_held(crate::cli::resolver::Route::Resolver(&route), sent)
            .await
            .map_err(refused)?
            .map_err(did_not_answer)?;
    let status = response.status();
    let body = response.text().await.map_err(did_not_answer)?;
    if !status.is_success() {
        let detail = match serde_json::from_str::<Value>(&body) {
            Ok(value) => match value.get("error").and_then(Value::as_str) {
                Some(error) => error.to_string(),
                None => format!("the resolver at {url} answered {status}: {body}"),
            },
            Err(error) => format!(
                "the resolver at {url} answered {status} with a body that is not JSON \
                 ({error}): {body}"
            ),
        };
        return Err(refused(detail));
    }
    // Checked as every reader of the authority checks it, so this command
    // never prints a document its readers would refuse.
    let payload: SnapshotPayload = serde_json::from_str(&body).map_err(|error| {
        refused(format!(
            "the resolver at {url} answered no registry snapshot ({error}): {body}"
        ))
    })?;
    let (document, store_version, _) = validate_snapshot(payload).map_err(CmdError::declaration)?;
    println!(
        "{}",
        serde_json::to_string(&SnapshotPayload {
            store_version,
            document,
        })?
    );
    Ok(())
}
