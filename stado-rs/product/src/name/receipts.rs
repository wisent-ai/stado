//! A machine's installation receipts follow a product's id.
//!
//! Receipts live under `~/.stado/products/<id>/<surface>.json` and each names
//! its product, so a renamed product would read as never installed and its
//! files as owned by nobody. `name set` moves this machine's receipts; every
//! other machine moves its own on its next `stado product sync` pass, or with
//! `stado product name adopt`: each reads the catalog's `former_names` and
//! moves a receipt directory still kept under a former id to the product's
//! current id. Nothing moves while an installation of the product holds one
//! of its surface locks, and nothing is merged into a directory the current
//! id already has: both are refused naming what holds them.

use crate::{
    catalog::{self, text},
    common::{emit, files::present, Runtime},
    state::{self, ProductState},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

fn root(runtime: &Runtime, id: &str) -> PathBuf {
    runtime.home.join(".stado/products").join(id)
}

/// Move the receipt directory of `former` to `id` and rewrite each receipt to
/// name `id`; one row per receipt moved, none when `former` has none here.
pub(crate) fn rename(runtime: &Runtime, former: &str, id: &str) -> Result<Vec<Value>> {
    let from = root(runtime, former);
    if !present(&from)? {
        return Ok(Vec::new());
    }
    let to = root(runtime, id);
    if present(&to)? {
        bail!(
            "{} and {} both hold receipts: the catalog names {former} a former id of {id}, so the \
             receipts under {former} belong to {id}; remove the one that records no installation \
             (`stado product status {id} --surface SURFACE --json` reads it) and run `stado \
             product name adopt` again",
            from.display(),
            to.display()
        );
    }
    let mut states = Vec::new();
    for entry in fs::read_dir(&from).with_context(|| format!("reading {}", from.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let surface = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .with_context(|| format!("{} is not a UTF-8 receipt name", path.display()))?
            .to_owned();
        // A held surface lock is a live writer; an after-install marker whose
        // process has ended is history the receipt keeps, not a writer.
        let holder = crate::install::operation::running(runtime, former, &surface)?;
        let live = !holder.is_null()
            && (holder["phase"] != "after_install" || holder["running"] == true);
        if live {
            bail!(
                "the {surface} installation of {former} is running ({holder}); its receipts move \
                 to {id} once it ends: run `stado product name adopt` then"
            );
        }
        let mut state = ProductState::load(runtime, former, &surface)?
            .with_context(|| format!("{} vanished while it was read", path.display()))?;
        state.product = id.to_owned();
        if let Some(previous) = state.previous.as_mut() {
            previous.product = id.to_owned();
        }
        states.push((surface, state));
    }
    #[allow(
        clippy::disallowed_methods,
        reason = "a whole receipt directory moves to the product's new id; no single document is written"
    )]
    fs::rename(&from, &to)
        .with_context(|| format!("moving {} to {}", from.display(), to.display()))?;
    let mut moved = Vec::new();
    for (surface, state) in states {
        state.save(runtime)?;
        moved.push(json!({
            "surface": surface,
            "from": from.join(format!("{surface}.json")),
            "to": state::path(runtime, id, &surface)?,
        }));
    }
    Ok(moved)
}

/// Every receipt directory this machine keeps under a former id of a product
/// in `document`, moved to the product's current id when `apply`; one row per
/// receipt, `moved` or `would-move`, and one per directory that could not
/// move, `refused` with the cause, so one held surface does not stop the rest.
pub(crate) fn adopt(runtime: &Runtime, document: &Value, apply: bool) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for product in super::products(document)? {
        let id = text(product, "id")?;
        for former in super::former_rows(product)? {
            let former = text(former, "id")?;
            if former == id || !present(&root(runtime, former))? {
                continue;
            }
            if !apply {
                rows.push(json!({"product": id, "from": root(runtime, former),
                    "to": root(runtime, id), "state": "would-move"}));
                continue;
            }
            match rename(runtime, former, id) {
                Ok(moved) => {
                    for mut row in moved {
                        row["product"] = json!(id);
                        row["state"] = json!("moved");
                        rows.push(row);
                    }
                }
                Err(error) => rows.push(json!({"product": id, "from": root(runtime, former),
                    "to": root(runtime, id), "state": "refused", "detail": format!("{error:#}")})),
            }
        }
    }
    Ok(rows)
}

/// `stado product name adopt`: exits 1 when a directory could not move.
pub(super) fn command(runtime: &Runtime, json: bool) -> Result<i32> {
    let document = catalog::current(runtime)?;
    let rows = adopt(runtime, &document, true)?;
    let refused = rows.iter().any(|row| row["state"] == "refused");
    if json {
        emit(&json!({"moved": rows}))?;
    } else if rows.is_empty() {
        println!("no receipt on this machine is kept under a former product id");
    } else {
        for row in &rows {
            let from = row["from"].as_str().context("an adopted receipt names where it was")?;
            let to = row["to"].as_str().context("an adopted receipt names where it goes")?;
            match row["detail"].as_str() {
                Some(detail) => println!("not moved {from} to {to}: {detail}"),
                None => println!("moved {from} to {to}"),
            }
        }
    }
    Ok(if refused {
        libc::EXIT_FAILURE
    } else {
        libc::EXIT_SUCCESS
    })
}
