//! A secret is asked for by what it is for, never by which item holds it.
//!
//! A release recipe, a job, an agent's field allowlist and a grant name a
//! role; the vault decides which item plays it by carrying the registered tag
//! `stado:role:<role>`. The item's id is read only after the selection, to
//! address the one read, so renaming an item changes nothing anywhere.

use super::ItemInfo;

/// The Skarbiec tag namespace Stado selects secrets by.
pub const ROLE_TAG_PREFIX: &str = "stado:role:";

/// The tag an item carries to play `role`.
pub fn role_tag(role: &str) -> String {
    format!("{ROLE_TAG_PREFIX}{role}")
}

/// The one command that stores the item playing `role` on `host`'s vault:
/// the item document piped into `stado credentials item put`, which takes
/// the role by `--role` and refuses a document whose `kind` is not its
/// `--type`. Every refusal and readiness report that tells an operator to
/// store a role's secret prints this, so the printed step is the one the
/// command accepts; `value` marks where the secret goes.
pub fn put_command(host: &str, role: &str, kind: &str, field: &str, value: &str) -> String {
    let document = serde_json::json!({
        "schema": "skarbiec.item.v2",
        "kind": kind,
        "fields": { field: value },
        "context": {},
    });
    format!(
        "printf '%s' '{document}' | stado credentials item put --host {host} --role {role} \
         --type {kind}"
    )
}

fn live(item: &ItemInfo) -> bool {
    !item.deleted
}

fn plays(item: &ItemInfo, tag: &str) -> bool {
    item.tags
        .as_deref()
        .is_some_and(|tags| tags.iter().any(|each| each == tag))
}

/// Every live item among `items` that plays `role`.
pub fn holders<'a>(items: &'a [ItemInfo], role: &str) -> Vec<&'a ItemInfo> {
    let tag = role_tag(role);
    items
        .iter()
        .filter(|item| live(item) && plays(item, &tag))
        .collect()
}

/// The id a new item is stored under: random, so it carries no meaning a
/// reader could come to depend on.
pub fn fresh_item_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The one live item among `items` that plays `role`. None and several are
/// both refusals: a missing role cannot be read, and two items in one role
/// leave the choice to listing order, which is a guess.
pub fn item_for_role<'a>(items: &'a [ItemInfo], role: &str) -> Result<&'a ItemInfo, String> {
    let tag = role_tag(role);
    match holders(items, role).as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "no item visible here carries {tag}; store the secret for role {role} with `{}`",
            put_command("<vault owner>", role, "<kind>", "<field>", "<secret>")
        )),
        several => Err(format!(
            "{} items carry {tag}; exactly one item may play role {role}",
            several.len()
        )),
    }
}

/// The read capabilities a grant needs for `role#field` entries: each role is
/// translated to the item that plays it at the moment the grant is issued, so
/// no configuration ever holds an item id.
pub fn read_capabilities(items: &[ItemInfo], entries: &[String]) -> Result<Vec<String>, String> {
    entries
        .iter()
        .map(|entry| {
            let (role, field) = entry
                .split_once('#')
                .filter(|(role, field)| !role.is_empty() && !field.is_empty())
                .ok_or_else(|| format!("{entry:?} is not role#field"))?;
            let item = item_for_role(items, role)?;
            Ok(format!("read:{}#{field}", item.id))
        })
        .collect()
}

/// What a grant exposes, as roles: every role a live item plays, sorted, with
/// an item that plays none shown as `(no role)` so an exposure no
/// configuration asked for is still visible.
pub fn roles_played(items: &[ItemInfo]) -> Vec<String> {
    let mut played: Vec<String> = items
        .iter()
        .filter(|item| live(item))
        .flat_map(|item| {
            let roles: Vec<String> = item
                .tags
                .iter()
                .flatten()
                .filter_map(|tag| tag.strip_prefix(ROLE_TAG_PREFIX))
                .map(str::to_string)
                .collect();
            if roles.is_empty() {
                vec!["(no role)".to_string()]
            } else {
                roles
            }
        })
        .collect();
    played.sort();
    played
}
