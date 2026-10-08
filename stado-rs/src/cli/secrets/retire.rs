//! `stado credentials vault retire-copy PATH --owner HOST [--apply]`: retire
//! one vault file a client machine still holds.
//!
//! The fleet keeps one vault, on its owner. A machine that reads the owner
//! through `secrets.skarbiec.url` and declares no `secrets.skarbiec.vault_file`
//! holds none, so every vault file left on it is a copy nobody reads — and a
//! copy can still hold the only instance of an item: a credential written
//! into it while something on the machine still treated it as the vault.
//! Deleting the file then deletes that credential. This command compares the
//! copy with the owner item by item, moves every item the owner lacks or holds
//! older onto the owner (`skarbiec set-json` there, the write `credentials item
//! put` uses), proves the owner now holds all of them, and only then removes
//! the file. Without `--apply` it only reports.

use serde_json::{json, Value};

use crate::cli::secrets::store::inventory::vault_items;
use crate::cli::secrets::store::resolve::{launcher_json, skarbiec_launcher};
use crate::cli::CmdError;

/// Skarbiec's `set-json` refusal of a payload another live item already
/// holds (`<holder> already holds exactly this payload; an exact duplicate is
/// refused …`), the owner's own content comparison.
const DUPLICATE_REFUSAL: &str = "already holds exactly this payload";

/// One live item of the copy that the owner does not hold as it stands here.
struct Missing {
    id: String,
    /// The id the owner holds the item under: its own after a rename, the
    /// copy's when the owner lacks it.
    owner_id: String,
    kind: Option<String>,
    tags: Vec<String>,
    reason: &'static str,
    copy_updated_at: Option<String>,
    owner_updated_at: Option<String>,
}

/// The comparison of one copy with the owner vault.
struct Comparison {
    held: Vec<String>,
    missing: Vec<Missing>,
}

/// What one run found and did.
struct Retirement<'a> {
    copy: &'a str,
    owner_host: &'a str,
    owner_vault: &'a str,
    comparison: &'a Comparison,
    moved: &'a [Value],
    /// Copy items the owner refused as exact duplicates of an item it holds
    /// under another id, with the owner's own sentence naming that item.
    duplicates: &'a [Value],
    /// Copy items this run could not write to the owner, each with why; the
    /// copy is kept while any remain.
    not_moved: &'a [Value],
    removed: bool,
}

fn text(item: &Value, key: &str) -> Option<String> {
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
fn compare(copy: &[Value], owner: &[Value]) -> Comparison {
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
        comparison.missing.push(Missing {
            kind: text(item, "kind"),
            owner_updated_at: on_owner.and_then(|theirs| text(theirs, "updated_at")),
            owner_id: on_owner
                .and_then(|theirs| text(theirs, "id"))
                .map_or_else(|| id.clone(), |theirs| theirs),
            id,
            tags,
            reason,
            copy_updated_at,
        });
    }
    comparison
}

/// The owner recorded for one vault file by `skarbiec vaults`.
fn owner_of(vaults: &Value, path: &str) -> Option<String> {
    vaults
        .get("vaults")
        .and_then(Value::as_array)?
        .iter()
        .find(|vault| text(vault, "path").as_deref() == Some(path))
        .and_then(|vault| text(vault, "owner"))
}

fn print(run: &Retirement<'_>, json_output: bool) -> Result<(), CmdError> {
    if json_output {
        let document = json!({
            "copy": run.copy,
            "owner": run.owner_host,
            "owner_vault": run.owner_vault,
            "held_by_owner": run.comparison.held.len(),
            "missing_on_owner": run.comparison.missing.iter().map(|item| json!({
                "id": item.id,
                "owner_id": item.owner_id,
                "kind": item.kind,
                "reason": item.reason,
                "copy_updated_at": item.copy_updated_at,
                "owner_updated_at": item.owner_updated_at,
            })).collect::<Vec<Value>>(),
            "moved": run.moved,
            "held_under_another_id": run.duplicates,
            "not_moved": run.not_moved,
            "removed": run.removed,
        });
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }
    println!("copy:     {}", run.copy);
    println!("owner:    {} ({})", run.owner_host, run.owner_vault);
    println!("held:     {}", run.comparison.held.len());
    println!("missing on the owner: {}", run.comparison.missing.len());
    for item in &run.comparison.missing {
        println!("  {:<60} {}", item.id, item.reason);
    }
    println!("moved:    {}", run.moved.len());
    println!("held under another id: {}", run.duplicates.len());
    for item in run.not_moved {
        println!("not moved: {} — {}", item["id"], item["why"]);
    }
    println!("removed:  {}", run.removed);
    Ok(())
}

pub(crate) async fn retire_copy(
    path: &str,
    owner_host: &str,
    apply: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let declared = crate::config::skarbiec_vault_file();
    if !declared.trim().is_empty() {
        return Err(CmdError::refused(format!(
            "this machine declares its own vault in secrets.skarbiec.vault_file ({declared}); \
             retire-copy retires a copy on a machine that reads the owner through \
             secrets.skarbiec.url and holds no vault"
        )));
    }
    if crate::config::skarbiec_url().trim().is_empty() {
        return Err(CmdError::refused(
            "this machine declares neither secrets.skarbiec.vault_file nor secrets.skarbiec.url, \
             so no owner reads for it and its vault files are not copies of anything",
        ));
    }
    let file = std::path::Path::new(path);
    if !file.is_file() {
        return Err(CmdError::click(format!("no vault file at {path}"))
            .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    let launcher = skarbiec_launcher()?;
    let local_vaults = launcher_json(&launcher, file, &["vaults"])?;
    let Some(copy_owner) = owner_of(&local_vaults, path) else {
        return Err(CmdError::refused(format!(
            "skarbiec vaults does not list {path} as a vault file on this machine, so it is not \
             a vault copy this command can retire"
        )));
    };

    let target = crate::cli::canonical_host(owner_host).await?;
    let runner = crate::deploy::production_runner();
    let broker = crate::deploy::host_capability::resolve(&target, &Default::default(), &runner)
        .await
        .map_err(CmdError::from)?;
    let owner_vaults = crate::deploy::fleet_vaults::collect_from(&target, &runner).await;
    let Some(fleet_owner) = owner_of(&owner_vaults, &broker.vault) else {
        return Err(CmdError::unreachable(format!(
            "{}: skarbiec vaults there does not list the owner vault {}",
            target.name, broker.vault
        )));
    };
    if fleet_owner != copy_owner {
        return Err(CmdError::refused(format!(
            "{path} belongs to owner {copy_owner}, and the vault on {} belongs to {fleet_owner}: \
             it is a different vault, not a copy of the fleet's, and nothing is moved or removed",
            target.name
        )));
    }

    let copy = vault_items(&launcher, file)?;
    let owner = crate::deploy::host_capability::items_with_trash(&target, &broker, &runner)
        .await
        .map_err(CmdError::from)?;
    let comparison = compare(&copy, &owner);
    if !apply {
        return print(
            &Retirement {
                copy: path,
                owner_host: &target.name,
                owner_vault: &broker.vault,
                comparison: &comparison,
                moved: &[],
                duplicates: &[],
                not_moved: &[],
                removed: false,
            },
            json_output,
        );
    }

    let mut moved = Vec::new();
    let mut duplicates = Vec::new();
    let mut not_moved = Vec::new();
    for item in &comparison.missing {
        // What the owner's set-json cannot take is named and left in the
        // copy, which then stays: a legacy envelope has no kind until the
        // copy is upgraded, and an id outside the host channel's alphabet (a
        // credential-lifecycle record such as `directory:credential/<name>`)
        // is not a write this path may send.
        let Some(kind) = item.kind.as_deref() else {
            not_moved.push(json!({
                "id": item.id,
                "why": format!("legacy envelope with no kind; run `SKARBIEC_VAULT_FILE={path} skarbiec upgrade --apply` and retire again"),
            }));
            continue;
        };
        if let Err(refused) = crate::cli::host::vault_word("vault item", &item.owner_id) {
            not_moved.push(json!({ "id": item.id, "why": refused.message }));
            continue;
        }
        let payload = launcher_json(&launcher, file, &["get", item.id.as_str()])?;
        // An item the owner lacks arrives with the copy's tags; one the owner
        // holds older keeps the tags the owner gave it.
        let tags = item.tags.join(",");
        let written = crate::cli::host::write_vault_item(
            &target.name,
            &item.owner_id,
            kind,
            &serde_json::to_string(&payload)?,
            false,
            (item.reason == "only_in_copy").then_some(tags.as_str()),
        )
        .await;
        match written {
            Ok(written) => moved.push(written),
            // The owner's Skarbiec refuses a payload one of its live items
            // already holds, naming that item: the owner has this secret under
            // another id (a retired name the copy still carries), which is the
            // proof by content no metadata comparison can give.
            Err(refused)
                if refused
                    .message
                    .as_deref()
                    .is_some_and(|said| said.contains(DUPLICATE_REFUSAL)) =>
            {
                duplicates.push(json!({ "id": item.id, "owner_says": refused.message }));
            }
            // Any other refusal by the owner is that item's answer, not the
            // run's: it is named, the copy is kept, and the rest still move.
            Err(refused) => not_moved.push(json!({ "id": item.id, "why": refused.message })),
        }
    }

    // Removal waits for proof read from the owner, not for the writes'
    // answers: the owner must now hold every live copy item at the same or a
    // later time, or hold its payload under another id.
    let owner = crate::deploy::host_capability::items_with_trash(&target, &broker, &runner)
        .await
        .map_err(CmdError::from)?;
    let mut after = compare(&copy, &owner);
    after.missing.retain(|item| {
        !duplicates
            .iter()
            .any(|duplicate| duplicate["id"].as_str() == Some(item.id.as_str()))
    });
    let kept = !after.missing.is_empty();
    if !kept {
        std::fs::remove_file(file)?;
    }
    print(
        &Retirement {
            copy: path,
            owner_host: &target.name,
            owner_vault: &broker.vault,
            comparison: &after,
            moved: &moved,
            duplicates: &duplicates,
            not_moved: &not_moved,
            removed: !kept,
        },
        json_output,
    )?;
    if kept && !not_moved.is_empty() {
        return Err(CmdError::refused(format!(
            "{} item(s) of {path} could not be written to {} (each is named under not_moved with \
             the reason), so {path} was kept",
            not_moved.len(),
            target.name
        )));
    }
    if kept {
        return Err(CmdError::unreachable(format!(
            "{} item(s) of {path} are still missing on {} after the move, so {path} was kept",
            after.missing.len(),
            target.name
        )));
    }
    Ok(())
}
