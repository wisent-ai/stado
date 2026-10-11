//! The forward markers the directory declares for one machine, and the
//! keeper that writes the ones missing or holding another address.

use serde_json::{json, Value};

use crate::cli::directory::document::{directory, endpoint_url, DIRECTORY_KEY};
use crate::cli::directory::report::markers::{adapter_url, write_forward_marker};

/// One marker the directory gives this machine an address for.
pub(crate) struct DeclaredMarker {
    pub(crate) service: String,
    pub(crate) url: String,
    pub(crate) source: &'static str,
}

/// Every marker the directory declares for one machine, and every service it
/// names that this machine gets no address for, with the reason.
pub(crate) struct DeclaredMarkers {
    pub(crate) addressed: Vec<DeclaredMarker>,
    pub(crate) skipped: Vec<(String, String)>,
}

/// What the directory in `document` says `target` dials for each service:
/// the directory's own endpoint for that machine, otherwise the bind of the
/// one resolver adapter it declares for the service.
///
/// The operator's `publish` and the resolver's keeper both answer from this,
/// so the markers a host carries cannot depend on which of the two wrote last.
pub(crate) fn declared_markers(document: &Value, target: &str) -> Result<DeclaredMarkers, String> {
    let block = directory(document).map_err(|error| error.to_string())?;
    let services = block
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{DIRECTORY_KEY}.services: must be an object"))?;
    // The target's own declaration, for the services the directory places
    // elsewhere. Read once: the adapter set does not change inside one run.
    let target_entry = document
        .get("targets")
        .and_then(Value::as_array)
        .and_then(|targets| {
            targets
                .iter()
                .find(|candidate| candidate.get("name").and_then(Value::as_str) == Some(target))
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut markers = DeclaredMarkers {
        addressed: Vec::new(),
        skipped: Vec::new(),
    };
    for (name, entry) in services {
        if let Some(url) = endpoint_url(entry, target) {
            markers.addressed.push(DeclaredMarker {
                service: name.clone(),
                url: url.to_string(),
                source: "directory-endpoint",
            });
            continue;
        }
        match adapter_url(&target_entry, name) {
            Ok(Some(url)) => markers.addressed.push(DeclaredMarker {
                service: name.clone(),
                url,
                source: "resolver-adapter",
            }),
            Ok(None) => markers.skipped.push((
                name.clone(),
                format!(
                    "{DIRECTORY_KEY} declares no endpoint for {target} and its resolver declares \
                     no {name} adapter"
                ),
            )),
            Err(reason) => markers.skipped.push((name.clone(), reason)),
        }
    }
    Ok(markers)
}

/// `~/.stado/forwards`, where every marker lives.
pub(super) fn forwards_dir() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(std::path::Path::new(&home).join(".stado").join("forwards"))
}

/// Write every marker the directory declares for `target` whose file is
/// missing or holds another address, and name each one written.
///
/// The resolver calls this whenever it loads the directory. Markers were
/// written only by an operator running `stado service directory publish` on
/// that machine, so a host where nobody had was missing them for good: the
/// vault host had no `brama.local` while the directory declared Brama on it,
/// and Weles, which reads that file at startup, exited on every launch. A
/// marker already holding the declared address is left alone, so a refresh
/// that changes nothing writes nothing; undeclared markers stay a finding for
/// `publish`, never a deletion here.
pub(crate) fn keep_declared_markers(document: &Value, target: &str) -> Result<Vec<String>, String> {
    let markers = declared_markers(document, target)?;
    let forwards = forwards_dir()?;
    std::fs::create_dir_all(&forwards)
        .map_err(|error| format!("cannot create {}: {error}", forwards.display()))?;
    let mut written = Vec::new();
    for marker in &markers.addressed {
        let path = forwards.join(format!("{}.local", marker.service));
        // Only a missing marker is absent; one that exists but cannot be
        // read is refused with its cause instead of being overwritten blind.
        match std::fs::read_to_string(&path) {
            Ok(current) if current.trim() == marker.url => continue,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        }
        write_forward_marker(&path, &marker.url)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        written.push(format!("{} -> {}", marker.service, marker.url));
    }
    Ok(written)
}
