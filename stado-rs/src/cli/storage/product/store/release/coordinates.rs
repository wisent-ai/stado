//! Every `(version, platform)` coordinate the release channel holds.

use crate::cli::storage::*;

/// One `(version, platform)` coordinate the release channel holds, with the
/// object names it actually carries.
///
/// The names come along because the audit's questions are about the SET, not
/// about any one object: "is this whole", "is this only a claim". They are
/// already in the listing this walk performs, so carrying them costs nothing
/// and saves the caller a second walk.
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
}

impl PublishedCoordinate {
    /// A coordinate that holds its claim and nothing else.
    ///
    /// The claim is written create-only BEFORE any artifact, by every
    /// publisher, so this is the state of a publication that stated which
    /// build it was and then wrote no bytes. It is not a partial coordinate:
    /// there is nothing to be short of yet, so no object-level audit can say
    /// more than "absent".
    pub fn claim_only(&self) -> bool {
        let claim_name = if self.version_scope {
            crate::release_control::RELEASE_VERSION_REVISION_NAME
        } else {
            crate::release_control::RELEASE_REVISION_NAME
        };
        let mut names = self.names.iter();
        matches!((names.next(), names.next()), (Some(only), None) if only == claim_name)
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
    // Keys, whichever store answers. The authenticated list route when the
    // object API is configured; the backend's own listing otherwise, so the
    // audit still runs on a host holding its releases locally rather than
    // reporting that it could not look.
    let mut keys: Vec<String> = Vec::new();
    match RemoteObjectApi::configured_for_list("releases", &prefix)? {
        Some(remote) => {
            for entry in remote.list("releases", &prefix).await? {
                // An entry without its key cannot be audited, so it is
                // refused with what the route sent, not left out of the count.
                let Some(key) = entry.get("key").and_then(Value::as_str) else {
                    return Err(CmdError::click(format!(
                        "the object API listed a release object under {prefix} without its key: \
                         {entry}"
                    ))
                    .stating(crate::primitives::failure::FailureCode::InfraDown));
                };
                keys.push(key.to_string());
            }
        }
        None => {
            let store = JobStorage::for_object_uris().await?;
            let namespaced =
                crate::remote::object_store::ObjectRef::namespace_prefix("releases", &prefix)?;
            // Names only: the audit reads no size or metadata.
            for name in store
                .backend()
                .list_paths(&namespaced, crate::queue::Listing::Every)
                .await?
            {
                let object = crate::remote::object_store::ObjectRef::from_storage_path(&name)?;
                keys.push(object.key().to_string());
            }
        }
    }
    let mut seen: BTreeMap<(String, String), PublishedCoordinate> = BTreeMap::new();
    let mut version_claims: BTreeSet<String> = BTreeSet::new();
    for key in keys {
        // Residue from an interrupted multipart upload is not a published
        // object. `stado storage put` stages parts below this suffix and only
        // promotes the complete object.
        if key.contains(".__stado_upload/") {
            continue;
        }
        // The listing is of `<product>/`, so a key outside it is the store
        // answering another question, refused with the key it gave.
        let Some(below) = key.strip_prefix(&prefix) else {
            return Err(CmdError::click(format!(
                "the release listing of {prefix} answered the key {key}, which is not under it"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        };
        let parts: Vec<&str> = below.split('/').collect();
        match parts.as_slice() {
            [version] => {
                coordinate(&mut seen, version, "")
                    .names
                    .insert("<version-root-object>".to_string());
            }
            [version, name] => {
                if *name == crate::release_control::RELEASE_VERSION_REVISION_NAME {
                    version_claims.insert(version.to_string());
                } else {
                    coordinate(&mut seen, version, "")
                        .names
                        .insert(name.to_string());
                }
            }
            // `<version>/<platform>/<name>`, the name holding any further
            // segments.
            [version, platform, name @ ..] => {
                coordinate(&mut seen, version, platform)
                    .names
                    .insert(name.join("/"));
            }
            // `split` yields at least one segment.
            [] => {}
        }
    }
    let versions_with_platforms: BTreeSet<String> = seen
        .values()
        .filter(|coordinate| !coordinate.version_scope)
        .map(|coordinate| coordinate.version.clone())
        .collect();
    for coordinate in seen.values_mut() {
        if !coordinate.version_scope && version_claims.contains(&coordinate.version) {
            coordinate.has_version_claim = true;
        }
    }
    for version in version_claims {
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
            });
        entry.has_version_claim = true;
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

/// The coordinate `(version, platform)` among `seen`, added with no names
/// when the walk meets it first; an empty platform is the version's own
/// scope.
fn coordinate<'a>(
    seen: &'a mut BTreeMap<(String, String), PublishedCoordinate>,
    version: &str,
    platform: &str,
) -> &'a mut PublishedCoordinate {
    seen.entry((version.to_string(), platform.to_string()))
        .or_insert_with(|| PublishedCoordinate {
            version: version.to_string(),
            platform: platform.to_string(),
            version_scope: platform.is_empty(),
            has_version_claim: false,
            names: BTreeSet::new(),
        })
}
