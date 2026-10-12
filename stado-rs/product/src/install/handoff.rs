//! An installation of the program running it.
//!
//! The steps after Stado's own build — signing, staging, placement and
//! activation — are code too, and the installed Stado that started the
//! install is the old code: a repair to any of them reaches this host only if
//! the build it repairs runs them. So once the build of the running product
//! passed and is retained, the installation stops, releases its writer lock,
//! and has the freshly built binary finish it with `stado product finish`,
//! which resumes from the retained build. The receipt names the binary that
//! finished it under `finished_by`.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use crate::{
    common::{capture, Runtime},
    state::ProductState,
};

/// The verb the built binary finishes a retained build with.
const FINISH: &str = "finish";

/// The product `stado product finish` finishes, for any other verb none.
pub(crate) fn finishing(action: &str, positional: &[String]) -> Option<String> {
    positional.first().filter(|_| action == FINISH).cloned()
}
/// The refusal of `stado product finish` for a revision no install retained.
pub(crate) fn nothing_retained(id: &str, revision: &str) -> anyhow::Error {
    anyhow!(
        "{id}: no build of {revision} is retained here, so `stado product {FINISH}` has \
         nothing to finish; `stado product update {id} --source-commit {revision}` builds it"
    )
}

/// The stage key of the program this build made that is the one running
/// now: the entry staged as `bin/<this executable's name>`.
pub(crate) fn staged_program(spec: &Value, id: &str) -> Result<String> {
    let running = std::env::current_exe().context("reading the running program's path")?;
    let name = running
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("the running program {} has no name", running.display()))?;
    let staged = format!("bin/{name}");
    let stage = spec["stage"]
        .as_object()
        .context("release platform has no stage map")?;
    match stage
        .iter()
        .find(|(_, member)| member.as_str() == Some(staged.as_str()))
    {
        Some((source, _)) => Ok(source.clone()),
        None => bail!(
            "{id}: the stage map stages no {staged}, so the program this build made cannot \
             finish the installation"
        ),
    }
}

/// The outcome of an installation frame: a [`HandOff`] its build returned
/// is finished now by the program the build made — after the frame let go of
/// the surface's writer lock — and any other outcome is the frame's own.
pub(crate) fn finished(
    result: Result<ProductState>,
    runtime: &Runtime,
    id: &str,
    surface: &str,
) -> Result<ProductState> {
    match result {
        Err(error) => match error.downcast::<HandOff>() {
            Ok(hand) => hand.finish(runtime, id, surface),
            Err(error) => Err(error),
        },
        installed => installed,
    }
}

/// The build of the running product passed and is retained; `binary` is that
/// build's own program, which finishes the installation.
#[derive(Debug)]
pub struct HandOff {
    pub binary: PathBuf,
    pub revision: String,
}

impl std::fmt::Display for HandOff {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the build of revision {} is retained for {} to finish",
            self.revision,
            self.binary.display()
        )
    }
}

impl std::error::Error for HandOff {}

impl HandOff {
    /// Run `<binary> product finish` for `id` on `surface`, after this
    /// process let go of the surface's writer lock, and read the receipt it
    /// recorded.
    pub fn finish(&self, runtime: &Runtime, id: &str, surface: &str) -> Result<ProductState> {
        let mut command = Command::new(&self.binary);
        command
            .args([
                "product",
                FINISH,
                id,
                "--surface",
                surface,
                "--source-commit",
            ])
            .arg(&self.revision)
            .args(runtime.signing.arguments())
            .stdin(Stdio::null());
        eprintln!(
            "{id}: the build passed and is retained; {} finishes the installation \
             (stado product {FINISH})",
            self.binary.display()
        );
        let output = capture(&mut command)?;
        if !output.status.success() {
            bail!(
                "{} product {FINISH} {id} --surface {surface} --source-commit {} exited {}; the \
                 build stays retained, so repeating the installation resumes at the step that \
                 failed: {} {}",
                self.binary.display(),
                self.revision,
                output.status,
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let mut state = ProductState::load(runtime, id, surface)?.with_context(|| {
            format!(
                "{} product {FINISH} succeeded but recorded no installation of {id}",
                self.binary.display()
            )
        })?;
        state.extra.insert(
            "finished_by".to_owned(),
            json!({"binary": self.binary, "verb": format!("stado product {FINISH}")}),
        );
        state.save(runtime)?;
        Ok(state)
    }
}
