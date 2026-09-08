use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::secrets::vault::mirror::read::remote_skarbiec_json_at;
use crate::cli::host::secrets::vault::mirror::{
    mirror_items, remote_skarbiec_json, SKARBIEC_MIRROR_RELATIVE,
};

/// What `stado credentials vault sync --host TARGET` would do, without doing any of it.
///
/// This preview exists because the operation it previews is not a merge, and
/// its name invites everyone to read it as one. `skarbiec sync-pull` copies the
/// mirror file over the live vault whole (`net::sync`: "A pull replaces the
/// whole live vault"), and merging is refused by design because "mirror and
/// live vault may be encrypted to different recipient sets". The only guard is
/// a refusal when a live item id is absent from the mirror; an id present on
/// both sides with different content is replaced by the mirror's copy with no
/// comment at all. So the interesting number is not how many items would be
/// added — it is how many would be replaced, and which of those something on
/// the host reads.
///
/// The comparison is `skarbiec list` against the live vault and against the
/// mirror file, both read-only, both over the same host channel. It reports the
/// mirror **as it currently sits on the host**: `sync-pull` runs `git pull`
/// first, so a mirror the target has not fetched yet can carry more than this
/// says. That is stated in the output rather than papered over, because a
/// preview that silently assumed a fetch would be a preview of a different
/// operation.
async fn preview_vault_sync(target: &str, json_output: bool) -> Result<(), CmdError> {
    let list = vec![String::from("list")];
    let (resolved, live_report) = remote_skarbiec_json(target, &list).await?;
    let (_, mirror_report) =
        remote_skarbiec_json_at(target, &list, Some(SKARBIEC_MIRROR_RELATIVE), None, None).await?;
    let live = mirror_items(&live_report)?;
    let mirror = mirror_items(&mirror_report)?;

    let mut rows = Vec::new();
    let mut conflicts = 0usize;
    let mut lost = 0usize;
    let mut new = 0usize;
    let mut same = 0usize;
    for (id, mirrored) in &mirror {
        match live.get(id) {
            None => {
                new += 1;
                rows.push(json!({
                    "item": id,
                    "verdict": "new",
                    "mirror_revision": mirrored.revision,
                }));
            }
            Some(current)
                if current.revision == mirrored.revision
                    && current.updated_at == mirrored.updated_at =>
            {
                same += 1;
            }
            Some(current) => {
                conflicts += 1;
                rows.push(json!({
                    "item": id,
                    "verdict": "conflict",
                    "host_revision": current.revision,
                    "mirror_revision": mirrored.revision,
                    "host_updated_at": current.updated_at,
                    "mirror_updated_at": mirrored.updated_at,
                }));
            }
        }
    }
    // The same set Skarbiec's own `items_missing_from_mirror` computes, and for
    // the same reason it ignores tombstones: losing a soft-deleted item is not
    // losing data, and a pull that would drop a live one is refused outright.
    for (id, current) in &live {
        if current.deleted || mirror.contains_key(id) {
            continue;
        }
        lost += 1;
        rows.push(json!({
            "item": id,
            "verdict": "lost",
            "host_revision": current.revision,
        }));
    }

    let would_apply = lost == 0;
    let report = json!({
        "target": resolved.name,
        "mirror": format!("$HOME/{SKARBIEC_MIRROR_RELATIVE}"),
        "mirror_freshness": "as it sits on the host; sync-pull fetches first, so an unfetched mirror can carry more",
        "host_items": live.len(),
        "mirror_items": mirror.len(),
        "counts": {"new": new, "same": same, "conflict": conflicts, "lost": lost},
        "items": rows,
        "would_apply": would_apply,
        "detail": if would_apply {
            "sync-pull would replace the live vault file with the mirror; every shared item takes the mirror's copy"
        } else {
            "sync-pull would refuse: the live vault carries items the mirror does not"
        },
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}: {} host item(s), {} mirror item(s) — {new} new, {same} same, {conflicts} conflict, {lost} lost",
            resolved.name,
            live.len(),
            mirror.len()
        );
        for row in &rows {
            println!(
                "  {:<8} {}",
                row["verdict"].as_str().unwrap_or_default(),
                row["item"].as_str().unwrap_or_default()
            );
        }
        println!("  {}", report["detail"].as_str().unwrap_or_default());
    }
    if conflicts == 0 && lost == 0 {
        Ok(())
    } else {
        Err(CmdError::silent(1))
    }
}

/// Pull the encrypted Skarbiec mirror into TARGET's live vault.
///
/// Not a merge, whatever the name suggests. `skarbiec sync-pull` copies the
/// mirror over the live vault whole; Skarbiec backs the live vault up first
/// and refuses when a live item id is absent from the mirror, and Stado
/// deliberately exposes no force path. An id on both sides takes the mirror's
/// copy with no comment, which is why `--check` exists and should be run
/// first: it names every item that would be replaced and every one that would
/// be lost.
pub async fn sync_vault(target: &str, check: bool, json_output: bool) -> Result<(), CmdError> {
    if check {
        return preview_vault_sync(target, json_output).await;
    }
    let (resolved, report) = remote_skarbiec_json(target, &[String::from("sync-pull")]).await?;
    if report.get("ok").and_then(Value::as_bool) != Some(true) {
        let reason = report
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("sync_refused");
        let detail = report
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("Skarbiec refused to replace the live vault");
        let local_only = report
            .get("local_only_items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        let local_only = if local_only.is_empty() {
            String::new()
        } else {
            format!("; local-only items: {local_only}")
        };
        return Err(CmdError::click(format!(
            "{}: Skarbiec {reason}: {detail}{local_only}",
            resolved.name
        )));
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": resolved.name,
                "status": "vault_synced",
                "skarbiec": report,
            }))?
        );
    } else {
        println!("{}: vault synced", resolved.name);
    }
    Ok(())
}
