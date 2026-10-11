//! The comparison of one vault copy with the owner vault, item by item.

use serde_json::Value;

/// One live item of the copy that the owner does not hold as it stands here.
pub(super) struct Missing {
    pub(super) id: String,
    /// The id the owner holds the item under: its own after a rename, the
    /// copy's when the owner lacks it.
    pub(super) owner_id: String,
    pub(super) kind: Option<String>,
    pub(super) tags: Vec<String>,
    pub(super) reason: &'static str,
    pub(super) copy_updated_at: Option<String>,
    pub(super) owner_updated_at: Option<String>,
}

/// The comparison of one copy with the owner vault.
pub(super) struct Comparison {
    pub(super) held: Vec<String>,
    pub(super) missing: Vec<Missing>,
}

/// The string `key` of `item`, when it holds one.
pub(super) fn text(item: &Value, key: &str) -> Option<String> {
    item.get(key).and_then(Value::as_str).map(str::to_string)
}

fn trashed(item: &Value) -> bool {
    item.get("deleted")
        .and_then(Value::as_bool)
        .is_some_and(|deleted| deleted)
}

/// Which live copy items the owner lacks (`only_in_copy`) or holds with an
/// older `updated_at` (`newer_in_copy`), and which it already holds. An item
/// is the owner's when the owner holds its id or its `item_uid`: a rename
/// keeps the uid, so an item the owner renamed since the copy was taken is
/// held, not missing, and moving it would make two items of one secret. The
/// owner's trashed items count as held when they are not older: an item the
/// owner removed on purpose is not brought back.
pub(super) fn compare(copy: &[Value], owner: &[Value]) -> Comparison {
    let mut comparison = Comparison {
        held: Vec::new(),
        missing: Vec::new(),
    };
    for item in copy.iter().filter(|item| !trashed(item)) {
        let Some(id) = text(item, "id") else {
            continue;
        };
        let copy_updated_at = text(item, "updated_at");
        let uid = text(item, "item_uid");
        let on_owner = owner
            .iter()
            .find(|candidate| text(candidate, "id").as_deref() == Some(id.as_str()))
            .or_else(|| {
                uid.as_ref().and_then(|uid| {
                    owner
                        .iter()
                        .find(|candidate| text(candidate, "item_uid").as_ref() == Some(uid))
                })
            });
        let reason = match on_owner {
            None => "only_in_copy",
            Some(theirs) if copy_updated_at > text(theirs, "updated_at") => "newer_in_copy",
            Some(_) => {
                comparison.held.push(id);
                continue;
            }
        };
        let tags = item
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        // The owner's own id when it holds the item (a rename changed it),
        // the copy's when the owner lacks it.
        let owner_id = match on_owner.and_then(|theirs| text(theirs, "id")) {
            Some(theirs) => theirs,
            None => id.clone(),
        };
        comparison.missing.push(Missing {
            kind: text(item, "kind"),
            owner_updated_at: on_owner.and_then(|theirs| text(theirs, "updated_at")),
            owner_id,
            id,
            tags,
            reason,
            copy_updated_at,
        });
    }
    comparison
}
