//! A peer fleet as this fleet declares it, and where the declaration lives.
//!
//! One JSON document per peer under `state/market/peers/<name>.json` in this
//! fleet's own store — the same store every host of the fleet reads — so a
//! declaration made on one host is the declaration every host lends under.
//! The bearer is never part of it: the declaration names the Skarbiec role
//! whose item holds it, and the value is read where it is used. Whether the
//! origin is a transport Stado accepts is judged where the peer's store is
//! opened ([`crate::queue::StadoObjectBackend::new`]), by the same rule every
//! store URL of this product meets.

use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, Listing, StorageError};

/// Where the declarations live in this fleet's store.
pub const PEERS_PREFIX: &str = "state/market/peers/";

/// What a peer operation can fail with, each with the sentence the operator
/// reads.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    /// The request itself is malformed: a name, an origin, a namespace.
    #[error("{0}")]
    Usage(String),
    /// The named peer is not declared, or the declaration is unreadable.
    #[error("{0}")]
    NotFound(String),
    /// The request contradicts what is already declared or running.
    #[error("{0}")]
    Refused(String),
    /// The bearer for the peer did not resolve on this host.
    #[error("{0}")]
    Credential(String),
    /// This fleet's store, or the peer's, did not answer.
    #[error("{0}")]
    Storage(#[from] StorageError),
    /// A local file or process operation failed.
    #[error("{0}")]
    Local(String),
}

/// One peer fleet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerDeclaration {
    /// The peer's name in this fleet.
    pub name: String,
    /// The origin of the object API that serves the peer's queue.
    pub origin: String,
    /// The object namespace the peer's queue lives in.
    pub namespace: String,
    /// The role of the item whose `token` field is the peer's bearer.
    pub credential_role: String,
    /// The PEM authority that signs a private origin; none for a publicly
    /// signed or loopback origin.
    pub ca_file: Option<String>,
    /// When the peer was declared, and when the declaration last changed.
    pub declared_at: String,
    pub changed_at: String,
}

impl PeerDeclaration {
    /// Validate every field the operator supplied, before anything is
    /// written.
    pub fn validate(&self) -> Result<(), PeerError> {
        validate_name(&self.name)?;
        let origin = reqwest::Url::parse(self.origin.trim()).map_err(|error| {
            PeerError::Usage(format!(
                "--origin {:?} is not a URL: {error}; give the peer's object API origin, \
                 for example https://objects.example.com",
                self.origin
            ))
        })?;
        if origin.query().is_some()
            || origin.fragment().is_some()
            || !matches!(origin.path(), "" | "/")
        {
            return Err(PeerError::Usage(format!(
                "--origin {:?} must be a bare origin with no path, query or fragment",
                self.origin
            )));
        }
        crate::remote::object_store::ObjectRef::new(&self.namespace, "configuration-check")
            .map_err(|error| {
                PeerError::Usage(format!(
                    "--namespace {:?} is not an object namespace: {error}",
                    self.namespace
                ))
            })?;
        if self.credential_role.trim().is_empty()
            || self.credential_role.chars().any(char::is_whitespace)
        {
            return Err(PeerError::Usage(format!(
                "--credential-role {:?} must be one Skarbiec role name with no spaces",
                self.credential_role
            )));
        }
        if let Some(ca_file) = &self.ca_file {
            let path = crate::config_file::expand_tilde(ca_file);
            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_not_file) => {
                    return Err(PeerError::Usage(format!(
                        "--ca-file {} is not a file on this host",
                        path.display()
                    )))
                }
                Err(error) => {
                    return Err(PeerError::Usage(format!(
                        "--ca-file {} cannot be read on this host: {error}",
                        path.display()
                    )))
                }
            }
        }
        Ok(())
    }
}

/// A peer name: lowercase letters, digits and inner hyphens. It becomes an
/// object key, a role-lock name and a directory, so nothing else is taken.
pub fn validate_name(name: &str) -> Result<(), PeerError> {
    let shaped = !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if shaped {
        Ok(())
    } else {
        Err(PeerError::Usage(format!(
            "peer name {name:?} must be lowercase letters, digits and inner hyphens"
        )))
    }
}

fn path(name: &str) -> String {
    format!("{PEERS_PREFIX}{name}.json")
}

/// The declaration of `name`, or `None` when no peer of that name exists.
pub async fn load(store: &JobStorage, name: &str) -> Result<Option<PeerDeclaration>, PeerError> {
    validate_name(name)?;
    let key = path(name);
    let Some(text) = store.download_text(&key).await? else {
        return Ok(None);
    };
    serde_json::from_str(&text).map(Some).map_err(|error| {
        PeerError::NotFound(format!(
            "{key} does not hold a peer declaration: {error}; `stado market peer remove {name}` \
             retires it and `stado market peer add` declares it again"
        ))
    })
}

/// The declaration of `name`, refused with the declared names when absent.
pub async fn require(store: &JobStorage, name: &str) -> Result<PeerDeclaration, PeerError> {
    if let Some(declaration) = load(store, name).await? {
        return Ok(declaration);
    }
    let declared: Vec<String> = list(store)
        .await?
        .into_iter()
        .map(|peer| peer.name)
        .collect();
    let named = if declared.is_empty() {
        "no peer is declared".to_string()
    } else {
        format!("declared: {}", declared.join(", "))
    };
    Err(PeerError::NotFound(format!(
        "no peer fleet named {name} is declared ({named}); `stado market peer add {name} \
         --origin URL --namespace NS --credential-role ROLE` declares it"
    )))
}

/// Every declared peer, by name.
pub async fn list(store: &JobStorage) -> Result<Vec<PeerDeclaration>, PeerError> {
    let mut peers = Vec::new();
    for key in store.list_paths(PEERS_PREFIX, Listing::Every).await? {
        let Some(name) = key
            .strip_prefix(PEERS_PREFIX)
            .and_then(|rest| rest.strip_suffix(".json"))
        else {
            continue;
        };
        if let Some(peer) = load(store, name).await? {
            peers.push(peer);
        }
    }
    peers.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(peers)
}

/// Write `declaration` after validating it, and read it back, so the answer
/// reports the document the store holds.
pub async fn save(
    store: &JobStorage,
    declaration: &PeerDeclaration,
) -> Result<PeerDeclaration, PeerError> {
    declaration.validate()?;
    let body = serde_json::to_string_pretty(declaration)
        .map_err(|error| PeerError::Local(format!("the declaration cannot be encoded: {error}")))?;
    let key = path(&declaration.name);
    store.upload_text(&key, &body).await?;
    match load(store, &declaration.name).await? {
        Some(stored) if &stored == declaration => Ok(stored),
        Some(_) => Err(PeerError::Refused(format!(
            "{key} holds another declaration after the write: another writer changed it at the \
             same time; `stado market peer show {}` reads what it holds now",
            declaration.name
        ))),
        None => Err(PeerError::Storage(StorageError::Other(format!(
            "{key} was written and is absent when read back"
        )))),
    }
}

/// Retire the declaration of `name`, and confirm it is gone.
pub async fn remove(store: &JobStorage, name: &str) -> Result<(), PeerError> {
    let key = path(name);
    store.delete_blob(&key).await?;
    if store.download_text(&key).await?.is_some() {
        return Err(PeerError::Storage(StorageError::Other(format!(
            "{key} was deleted and is still present when read back"
        ))));
    }
    Ok(())
}
