//! Every `(version, platform)` coordinate the release channel holds.

use crate::cli::storage::*;

/// One `(version, platform)` coordinate the release channel holds, with the
/// object names it actually carries and when its claim was written.
///
/// The names come along because the audit's questions are about the SET, not
/// about any one object: "is this whole", "is this only a claim", "did two
/// publishers disagree". They are already in the listing this walk performs,
/// so carrying them costs nothing and saves the caller a second walk.
#[derive(Debug, Clone)]
pub(crate) struct PublishedCoordinate {
    pub version: String,
    pub platform: String,
    /// True for a synthetic entry representing a version claim that has no
    /// platform objects yet.
    pub version_scope: bool,
    /// Whether the version-scoped arbitration record exists for this version.
    pub has_version_claim: bool,
    /// Every object name directly under the coordinate prefix.
    pub names: BTreeSet<String>,
    /// When the version claim was written, falling back to the legacy
    /// platform claim for releases created before version-scoped arbitration.
    pub claim_written_at: Option<DateTime<Utc>>,
}

impl PublishedCoordinate {
    /// A coordinate that holds its claim and nothing else.
    ///
    /// The claim is written create-only BEFORE any artifact, by every
    /// publisher, so this is the state of a publication that stated which
    /// build it was and then wrote no bytes. It is not a partial coordinate:
    /// there is nothing to be short of yet, and `SHA256SUMS` — the object
    /// that declares what a complete coordinate holds — is exactly what is
    /// missing, so no object-level audit can say more than "absent".
    pub fn claim_only(&self) -> bool {
        let claim_name = if self.version_scope {
            crate::release_control::RELEASE_VERSION_REVISION_NAME
        } else {
            crate::release_control::RELEASE_REVISION_NAME
        };
        self.names.len() == 1 && self.names.contains(claim_name)
    }
}

/// Every coordinate the release channel actually holds for one product,
/// newest version first.
///
/// Derived from the store's own listing rather than from git tags. A tag is
/// created before publication and survives one that never completed, so a tag
/// list answers "what did someone intend" while this answers "what is there" —
/// and the gap between those two is where `stado/0.10.0/darwin-arm64` sat at 0
/// objects of 9 from April until it was found by accident.
pub(crate) async fn published_release_coordinates(
    product: &str,
) -> Result<Vec<PublishedCoordinate>, CmdError> {
    let prefix = format!("{product}/");
    // Keys and their timestamps, whichever store answers. The authenticated
    // list route when the object API is configured; the backend's own listing
    // otherwise, so the audit still runs on a host holding its releases
    // locally rather than reporting that it could not look.
    let keys: Vec<(String, Option<DateTime<Utc>>)> =
        match RemoteObjectApi::configured_for_list("releases", &prefix)? {
            Some(remote) => remote
                .list("releases", &prefix)
                .await?
                .into_iter()
                .filter_map(|entry| {
                    let key = entry.get("key").and_then(Value::as_str)?.to_string();
                    let updated = entry
                        .get("updated_at")
                        .and_then(Value::as_str)
                        .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
                        .map(|stamp| stamp.with_timezone(&Utc));
                    Some((key, updated))
                })
                .collect(),
            None => {
                let store = JobStorage::new().await?;
                let namespaced =
                    crate::remote::object_store::ObjectRef::namespace_prefix("releases", &prefix)?;
                store
                    .backend()
                    .list_blobs_with_meta(&namespaced)
                    .await?
                    .into_iter()
                    .filter_map(|blob| {
                        crate::remote::object_store::ObjectRef::from_storage_path(&blob.name)
                            .ok()
                            .map(|object| (object.key().to_string(), blob.updated))
                    })
                    .collect()
            }
        };
    let mut seen: BTreeMap<(String, String), PublishedCoordinate> = BTreeMap::new();
    let mut version_claims: BTreeMap<String, Option<DateTime<Utc>>> = BTreeMap::new();
    for (key, updated) in keys {
        let key = key.as_str();
        // Residue from an interrupted multipart upload is not a published
        // object. `stado storage put` stages parts below this suffix and only
        // promotes the complete object.
        if key.contains(".__stado_upload/") {
            continue;
        }
        let parts: Vec<&str> = key.split('/').collect();
        if parts.len() == 2 && parts[0] == product {
            let entry = seen
                .entry((parts[1].to_string(), String::new()))
                .or_insert_with(|| PublishedCoordinate {
                    version: parts[1].to_string(),
                    platform: String::new(),
                    version_scope: true,
                    has_version_claim: false,
                    names: BTreeSet::new(),
                    claim_written_at: None,
                });
            entry.names.insert("<version-root-object>".to_string());
            continue;
        }
        if parts.len() == 3 && parts[0] == product {
            if parts[2] == crate::release_control::RELEASE_VERSION_REVISION_NAME {
                version_claims.insert(parts[1].to_string(), updated);
            } else {
                let entry = seen
                    .entry((parts[1].to_string(), String::new()))
                    .or_insert_with(|| PublishedCoordinate {
                        version: parts[1].to_string(),
                        platform: String::new(),
                        version_scope: true,
                        has_version_claim: false,
                        names: BTreeSet::new(),
                        claim_written_at: None,
                    });
                entry.names.insert(parts[2].to_string());
            }
            continue;
        }
        // `<product>/<version>/<platform>/<name>`; anything shorter is not a
        // platform coordinate.
        if parts.len() < 4 || parts[0] != product {
            continue;
        }
        let name = parts[3..].join("/");
        let entry = seen
            .entry((parts[1].to_string(), parts[2].to_string()))
            .or_insert_with(|| PublishedCoordinate {
                version: parts[1].to_string(),
                platform: parts[2].to_string(),
                version_scope: false,
                has_version_claim: false,
                names: BTreeSet::new(),
                claim_written_at: None,
            });
        if name == crate::release_control::RELEASE_REVISION_NAME {
            entry.claim_written_at = updated;
        }
        entry.names.insert(name);
    }
    let versions_with_platforms: BTreeSet<String> = seen
        .values()
        .filter(|coordinate| !coordinate.version_scope)
        .map(|coordinate| coordinate.version.clone())
        .collect();
    for coordinate in seen.values_mut() {
        if !coordinate.version_scope {
            if let Some(written) = version_claims.get(&coordinate.version) {
                coordinate.has_version_claim = true;
                coordinate.claim_written_at = *written;
            }
        }
    }
    for (version, written) in version_claims {
        if versions_with_platforms.contains(&version) {
            continue;
        }
        let entry = seen
            .entry((version.clone(), String::new()))
            .or_insert_with(|| PublishedCoordinate {
                version,
                platform: String::new(),
                version_scope: true,
                has_version_claim: false,
                names: BTreeSet::new(),
                claim_written_at: None,
            });
        entry.has_version_claim = true;
        entry.claim_written_at = written;
        entry
            .names
            .insert(crate::release_control::RELEASE_VERSION_REVISION_NAME.to_string());
    }
    let mut coordinates: Vec<PublishedCoordinate> = seen.into_values().collect();
    coordinates.sort_by(|left, right| {
        if left.version == right.version {
            return left.platform.cmp(&right.platform);
        }
        // `version_newer(a, b)` is "b is newer than a", so this asks whether
        // `left` is the newer version and puts it first.
        if crate::binary::release::version_newer(&right.version, &left.version) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    Ok(coordinates)
}
