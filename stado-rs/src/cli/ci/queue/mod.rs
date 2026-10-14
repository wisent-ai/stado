//! The delivery queue: the only way a commit reaches a product's `main`.
//!
//! A session pushes its work to the delivery branch of the product's
//! repository, never to `main`: the pre-push check refuses a push to `main`
//! of any commit this register does not record as admitted, and runs only
//! the compile-free checks on a push to the delivery branch, so no executor
//! builds. The queue's one serialized builder (`stado ci deliver`, and the
//! `stado serve --release-loop-root` role each time a checkout's view of its
//! origin moves) takes the delivery branch's tip and runs every installation
//! gate over that exact commit — the locks, the module declarations, every
//! format, lint and test gate the manifest declares and the CLI audit — each
//! to its end, so one run names every refusal. When none refuses, `main` is
//! moved to the candidate. When any does, `main` stays where it was and the
//! queue records one defect in Oko for the candidate with the whole list,
//! owned by the session whose recorded repair rides on it; the next
//! candidate that passes is recorded as that defect's repair. A candidate
//! that does not descend from `main` is refused before anything builds.
//! Every decision is kept in the register `stado ci status` prints.

pub mod oko;
pub mod register;

use std::path::{Path, PathBuf};

use chrono::Utc;
use fs2::FileExt;

use crate::cli::quality::Selection;
use crate::cli::release_loop::process::{run, succeeds};
use crate::cli::CmdError;
use crate::release_pipeline::PRODUCT_MANIFEST;
use register::{Entry, State};

/// The branch sessions push their candidates to. Git ref names are the
/// protocol between the pushing session, the forge and the builder.
pub const DELIVERY_BRANCH: &str = "delivery";
/// The branch only the queue moves.
pub const MAIN: &str = "main";

/// Where the checkout's origin holds `main` and the delivery branch.
pub struct Tips {
    pub main: Option<String>,
    pub delivery: Option<String>,
}

/// The tips of `main` and the delivery branch on the checkout's origin, as
/// the forge answers them now; `None` for a branch it does not hold.
pub async fn tips(checkout: &Path) -> Result<Tips, CmdError> {
    let main_head = format!("refs/heads/{MAIN}");
    let delivery_head = format!("refs/heads/{DELIVERY_BRANCH}");
    let listed = run(
        "git",
        &["ls-remote", "origin", &main_head, &delivery_head],
        Some(checkout),
    )
    .await?;
    let tip = |name: &str| {
        listed.lines().find_map(|line| {
            let (object, reference) = line.split_once('\t')?;
            (reference == name).then(|| object.to_string())
        })
    };
    Ok(Tips {
        main: tip(&main_head),
        delivery: tip(&delivery_head),
    })
}

/// The product `revision` of `checkout` declares in its committed manifest.
async fn product_of(checkout: &Path, revision: &str) -> Result<String, CmdError> {
    let manifest = run(
        "git",
        &["show", &format!("{revision}:{PRODUCT_MANIFEST}")],
        Some(checkout),
    )
    .await?;
    let declared: serde_json::Value = serde_json::from_str(&manifest).map_err(|error| {
        CmdError::refused(format!(
            "{PRODUCT_MANIFEST} at {revision} of {} is not JSON: {error}",
            checkout.display()
        ))
    })?;
    match declared["product"].as_str() {
        Some(product) => Ok(product.to_string()),
        None => Err(CmdError::refused(format!(
            "{PRODUCT_MANIFEST} at {revision} of {} names no product",
            checkout.display()
        ))),
    }
}

/// The one builder's lock, held for the whole pass: a pass that finds it
/// held waits, said on stderr, for the holder to finish.
fn serialized() -> Result<std::fs::File, CmdError> {
    let path = register::lock_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CmdError::click(format!("creating {}: {error}", parent.display())))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| CmdError::click(format!("opening {}: {error}", path.display())))?;
    crate::wait::blocking(
        crate::wait::Kind::Lock,
        "the delivery queue's builder lock, held by the pass gating a candidate",
        path.display(),
        || file.lock_exclusive(),
    )
    .map_err(|error| CmdError::click(format!("locking {}: {error}", path.display())))?;
    Ok(file)
}

/// Move `main` of the checkout's origin to the admitted `candidate`.
async fn advance(checkout: &Path, candidate: &str) -> Result<(), CmdError> {
    run(
        "git",
        &["push", "origin", &format!("{candidate}:refs/heads/{MAIN}")],
        Some(checkout),
    )
    .await
    .map(drop)
}

/// Record on every refused candidate of `checkout` that `admitted` carries
/// that `admitted` repairs it.
async fn repaired(checkout: &Path, admitted: &str) -> Result<(), CmdError> {
    for mut earlier in register::read()?.entries.into_iter().filter(|entry| {
        entry.checkout == checkout && entry.state == State::Refused && entry.repaired_by.is_none()
    }) {
        let carried = succeeds(
            "git",
            &["merge-base", "--is-ancestor", &earlier.candidate, admitted],
            checkout,
        )
        .await?;
        if !carried {
            continue;
        }
        if let Some(defect) = &earlier.defect {
            oko::record_repaired(defect, checkout, admitted).await?;
        }
        earlier.repaired_by = Some(admitted.to_string());
        earlier.updated_at = Utc::now();
        register::keep(&earlier)?;
    }
    Ok(())
}

/// One pass of the builder over `checkout`: the entry its candidate ended
/// with, or `None` when nothing waits (the origin holds no delivery branch,
/// or `main` already holds its tip).
pub async fn deliver(checkout: &Path) -> Result<Option<Entry>, CmdError> {
    let checkout: PathBuf = std::path::absolute(checkout).map_err(|error| {
        CmdError::click(format!(
            "cannot resolve the checkout {}: {error}",
            checkout.display()
        ))
    })?;
    let _builder = serialized()?;
    let tips = tips(&checkout).await?;
    let Some(main) = tips.main else {
        return Err(CmdError::refused(format!(
            "the origin of {} holds no {MAIN} branch for the queue to move",
            checkout.display()
        )));
    };
    let Some(candidate) = tips.delivery else {
        return Ok(None);
    };
    run(
        "git",
        &["fetch", "--quiet", "origin", MAIN, DELIVERY_BRANCH],
        Some(&checkout),
    )
    .await?;
    if succeeds(
        "git",
        &["merge-base", "--is-ancestor", &candidate, &main],
        &checkout,
    )
    .await?
    {
        return Ok(None);
    }
    if let Some(kept) = register::kept(&checkout, &candidate)? {
        match kept.state {
            State::Refused => return Ok(Some(kept)),
            // Admitted, but the move did not land: move it now.
            State::Admitted => {
                advance(&checkout, &candidate).await?;
                repaired(&checkout, &candidate).await?;
                return Ok(Some(kept));
            }
            // A pass that ended before deciding: decide it now.
            State::Running => {}
        }
    }
    let now = Utc::now();
    let mut entry = Entry {
        product: product_of(&checkout, &candidate).await?,
        checkout: checkout.clone(),
        candidate: candidate.clone(),
        main: main.clone(),
        state: State::Running,
        refusals: Vec::new(),
        defect: None,
        authors: oko::authors(&checkout, &main, &candidate).await?,
        repaired_by: None,
        started_at: now,
        updated_at: now,
    };
    register::keep(&entry)?;
    let descends = succeeds(
        "git",
        &["merge-base", "--is-ancestor", &main, &candidate],
        &checkout,
    )
    .await?;
    entry.refusals = match descends {
        true => {
            crate::cli::quality::refusals(&checkout, &candidate, Selection::Installation).await?
        }
        false => vec![format!(
            "{candidate} does not descend from {MAIN} at {main}: it was pushed from a base {MAIN} \
             has moved past; rebase it onto origin/{MAIN} and push the delivery branch again"
        )],
    };
    entry.updated_at = Utc::now();
    if entry.refusals.is_empty() {
        entry.state = State::Admitted;
        register::keep(&entry)?;
        advance(&checkout, &candidate).await?;
        repaired(&checkout, &candidate).await?;
        return Ok(Some(entry));
    }
    entry.state = State::Refused;
    entry.defect = Some(oko::record_refused(&entry).await?);
    register::keep(&entry)?;
    Ok(Some(entry))
}

/// `stado ci deliver --checkout DIR [--json]`: exits non-zero when the
/// candidate was refused, after printing the entry with every refusal.
pub async fn command(checkout: &Path, json: bool) -> Result<(), CmdError> {
    let entry = deliver(checkout).await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "entry": entry }))?
        );
    } else {
        match &entry {
            Some(entry) => println!("{}", register::line(entry)),
            None => println!(
                "{}: nothing waits; the origin's {DELIVERY_BRANCH} branch is absent or already on {MAIN}",
                checkout.display()
            ),
        }
    }
    match entry {
        Some(entry) if entry.state == State::Refused => Err(CmdError::refused(format!(
            "{} was refused by the delivery queue; {MAIN} stays at {}",
            entry.candidate, entry.main
        ))),
        _ => Ok(()),
    }
}

/// `stado ci status`'s queue section for `product`: every candidate the
/// register holds for it, newest first, and the origin's tips read from the
/// checkout of the newest one.
pub async fn status(product: &str) -> Result<serde_json::Value, CmdError> {
    let entries = register::of_product(product)?;
    let origin = match entries.first() {
        Some(newest) => {
            let tips = tips(&newest.checkout).await?;
            serde_json::json!({
                "checkout": newest.checkout,
                "main": tips.main,
                "delivery": tips.delivery,
            })
        }
        None => serde_json::Value::Null,
    };
    Ok(serde_json::json!({
        "register": register::path()?,
        "origin": origin,
        "entries": entries,
    }))
}
