//! The pushes a delivery pass owes, in the order a workspace full of
//! unpushed work needs them.
//!
//! Every push runs the repository's pre-push gate, and that gate is the
//! product the queue is about to deliver: pushing first meets the installed
//! gate as it was, which refuses what its new build accepts. So `stado
//! release newest --push --first P,…` delivers the `--first` products, installs
//! each on this host (`stado product install`), and only then pushes every
//! checkout of the workspace that holds commits its upstream does not, reads
//! the CI of each pushed product at the pushed commit (`stado ci status`), and
//! delivers the rest. The number of unpushed commits per checkout is measured
//! before and after, so the pass shows what it moved and what still waits.

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::register::{Entry, State};
use super::steps::process::run;
use crate::cli::release_newest::Planned;
use crate::cli::CmdError;

/// One checkout of the workspace and the commits it holds that its upstream
/// does not.
#[derive(Debug, Clone, Serialize)]
pub struct Unpushed {
    pub repository: String,
    #[serde(skip)]
    pub checkout: PathBuf,
    /// Commits on HEAD its upstream does not have; none when they could not
    /// be counted, with why in `unreadable`.
    pub ahead: Option<u64>,
    pub unreadable: Option<String>,
}

/// One `--first` product's installation on this host, or why it was not.
#[derive(Debug, Clone, Serialize)]
pub struct Installed {
    pub product: String,
    pub error: Option<String>,
}

/// One checkout's push, and the CI of its pushed commit.
#[derive(Debug, Clone, Serialize)]
pub struct Pushed {
    pub repository: String,
    pub ahead: u64,
    /// Why the push did not land; none when it did.
    pub error: Option<String>,
    /// `stado ci status` of the pushed commit, or why it was not read.
    pub ci: Option<String>,
    pub ci_unread: Option<String>,
}

/// The push phase of one pass: the measure before, the installations, the
/// pushes, and the measure after.
#[derive(Debug, Clone, Serialize)]
pub struct Phase {
    pub before: Vec<Unpushed>,
    pub installed: Vec<Installed>,
    pub pushed: Vec<Pushed>,
    pub after: Vec<Unpushed>,
}

/// Every git checkout directly under `root`, with its unpushed commits.
pub async fn unpushed(root: &Path) -> Result<Vec<Unpushed>, CmdError> {
    let listed = std::fs::read_dir(root).map_err(|error| {
        CmdError::click(format!(
            "cannot list the workspace {}: {error}",
            root.display()
        ))
    })?;
    let mut checkouts = Vec::new();
    for entry in listed {
        let path = entry
            .map_err(|error| {
                CmdError::click(format!(
                    "cannot list the workspace {}: {error}",
                    root.display()
                ))
            })?
            .path();
        if path.join(".git").exists() {
            checkouts.push(path);
        }
    }
    checkouts.sort();
    let mut found = Vec::new();
    for checkout in checkouts {
        let repository = checkout
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| {
                CmdError::click(format!("{} has no directory name", checkout.display()))
            })?;
        let (ahead, unreadable) = match run(
            "git",
            &["rev-list", "--count", "@{upstream}..HEAD"],
            Some(&checkout),
        )
        .await
        {
            Ok(said) => match said.trim().parse::<u64>() {
                Ok(count) => (Some(count), None),
                Err(error) => (
                    None,
                    Some(format!("git rev-list --count answered {said:?}: {error}")),
                ),
            },
            Err(error) => (None, Some(error.to_string())),
        };
        found.push(Unpushed {
            repository,
            checkout,
            ahead,
            unreadable,
        });
    }
    Ok(found)
}

/// This Stado's own executable, to run its product installation and CI read.
fn stado() -> Result<String, CmdError> {
    std::env::current_exe()
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| {
            CmdError::click(format!(
                "this Stado cannot name its own executable: {error}"
            ))
        })
}

/// Why `entry`, which failed a queue step, leaves its product uninstalled.
fn refused(entry: &Entry) -> String {
    let said = match &entry.error {
        Some(error) => format!(": {error}"),
        None => ", and the register kept no error for it".to_string(),
    };
    format!(
        "not installed: its commit {} failed the queue's {} step{said}",
        entry.commit,
        entry.step.name()
    )
}

/// Install each `leading` product on this host from its checkout, unless its
/// commit failed a queue step in `delivered`: a build the queue refused is not
/// the gate to push through.
pub async fn install(leading: &[Planned], delivered: &[Entry]) -> Result<Vec<Installed>, CmdError> {
    let stado = stado()?;
    let mut installed = Vec::new();
    for planned in leading {
        let failed = delivered
            .iter()
            .find(|entry| entry.product == planned.product && entry.state == State::Failed);
        let error = match failed {
            Some(entry) => Some(refused(entry)),
            None => run(&stado, &["product", "install", &planned.product], None)
                .await
                .err()
                .map(|e| e.to_string()),
        };
        installed.push(Installed {
            product: planned.product.clone(),
            error,
        });
    }
    Ok(installed)
}

/// Push every checkout of `before` that holds unpushed commits, once every
/// `installed` product is in place, and read each pushed product's CI.
pub async fn push(
    root: &Path,
    before: &[Unpushed],
    planned: &[Planned],
    installed: &[Installed],
) -> Result<Vec<Pushed>, CmdError> {
    let stado = stado()?;
    let missing: Vec<String> = installed
        .iter()
        .filter_map(|product| {
            product
                .error
                .as_ref()
                .map(|error| format!("{}: {error}", product.product))
        })
        .collect();
    let mut pushed = Vec::new();
    for checkout in before {
        let Some(ahead) = checkout
            .ahead
            .filter(|ahead| NonZeroU64::new(*ahead).is_some())
        else {
            continue;
        };
        if !missing.is_empty() {
            pushed.push(Pushed {
                repository: checkout.repository.clone(),
                ahead,
                error: Some(format!(
                    "not pushed: the products every push needs first are not installed ({})",
                    missing.join("; ")
                )),
                ci: None,
                ci_unread: Some("not pushed, so no CI was read".to_string()),
            });
            continue;
        }
        let error = run("git", &["push"], Some(&checkout.checkout))
            .await
            .err()
            .map(|error| error.to_string());
        let product = planned
            .iter()
            .find(|entry| root.join(&entry.checkout) == checkout.checkout);
        let (ci, ci_unread) = match (&error, product) {
            (Some(_), _) => (None, Some("not pushed, so no CI was read".to_string())),
            (None, None) => (
                None,
                Some(format!(
                    "{} declares no product, so no CI is read for it",
                    checkout.repository
                )),
            ),
            (None, Some(product)) => {
                match run("git", &["rev-parse", "HEAD"], Some(&checkout.checkout)).await {
                    Err(error) => (None, Some(error.to_string())),
                    Ok(head) => {
                        let args = [
                            "ci",
                            "status",
                            product.product.as_str(),
                            "--revision",
                            head.trim(),
                        ];
                        match run(&stado, &args, None).await {
                            Ok(said) => (Some(said), None),
                            Err(error) => (None, Some(error.to_string())),
                        }
                    }
                }
            }
        };
        pushed.push(Pushed {
            repository: checkout.repository.clone(),
            ahead,
            error,
            ci,
            ci_unread,
        });
    }
    Ok(pushed)
}

/// The phase as `stado release newest` prints it.
pub fn print(phase: &Phase) {
    for (when, measured) in [("before", &phase.before), ("after", &phase.after)] {
        for checkout in measured {
            match (checkout.ahead, &checkout.unreadable) {
                (Some(ahead), _) => println!("  unpushed {when}  {}  {ahead}", checkout.repository),
                (None, Some(why)) => {
                    println!("  unpushed {when}  {}  unread: {why}", checkout.repository)
                }
                (None, None) => println!("  unpushed {when}  {}  unread", checkout.repository),
            }
        }
    }
    for product in &phase.installed {
        match &product.error {
            None => println!("  installed  {}", product.product),
            Some(error) => println!("  installed  {}  refused: {error}", product.product),
        }
    }
    for checkout in &phase.pushed {
        match &checkout.error {
            None => println!(
                "  pushed  {}  {} commit(s)",
                checkout.repository, checkout.ahead
            ),
            Some(error) => println!("  pushed  {}  refused: {error}", checkout.repository),
        }
        match (&checkout.ci, &checkout.ci_unread) {
            (Some(ci), _) => println!("  ci  {}\n{}", checkout.repository, ci.trim_end()),
            (None, Some(why)) => println!("  ci  {}  unread: {why}", checkout.repository),
            (None, None) => println!("  ci  {}  unread", checkout.repository),
        }
    }
}
