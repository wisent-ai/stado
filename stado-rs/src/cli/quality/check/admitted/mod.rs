//! What a quality check runs that the operator's rulings govern, asked of
//! Oko's admission decision (`oko admit format|compile|test|build --product
//! P --purpose DEFECT`) before it runs: a format gate checks the tree with a
//! formatter, a lint gate compiles it, a test gate runs its tests, and
//! reading the command tree builds the CLI. A gate whose kind the rulings
//! rule out is left out of the check and said, with the ruling: which kinds
//! of work run is the installation's decision, made once in its rulings, not
//! in every product's release declaration. The operation that started the
//! check passes its admission down (`allowed`).

use crate::cli::release_loop::allowed::{refusal, ruling, BUILD, COMPILE, TEST};
use crate::cli::CmdError;
use crate::release_pipeline::GateKind;

/// The action class a ruling constrains that checking source against its
/// formatter is (a `format` quality gate).
const FORMAT: &str = "format";

/// The action a gate of `kind` takes.
fn action(kind: GateKind) -> &'static str {
    match kind {
        GateKind::Format => FORMAT,
        GateKind::Lint => COMPILE,
        GateKind::Test => TEST,
    }
}

/// `None` when gate `name` of kind `kind` runs on `revision` of `product` as
/// work serving `purposes`; `Some` with the ruling when the rulings rule its
/// kind out, so the check leaves it out and says why; the error when no
/// decision could be read, which runs nothing.
pub(super) async fn gate(
    kind: GateKind,
    name: &str,
    product: &str,
    revision: &str,
    purposes: &[&str],
) -> Result<Option<String>, CmdError> {
    match ruling(action(kind), Some(product), purposes).await {
        Ok(None) => Ok(None),
        Ok(Some(ruled_out)) => Ok(Some(format!("{ruled_out}; gate {name:?} of {revision} is left out"))),
        Err(unread) => Err(CmdError::refused(format!("{unread}; gate {name:?} of {revision} was not run"))),
    }
}

/// Ok when the CLI of `revision` of `product` may be built to read its
/// commands, as work serving `purposes`; else the refusal naming the ruling.
pub(super) async fn build(
    product: &str,
    revision: &str,
    purposes: &[&str],
) -> Result<(), CmdError> {
    refusal(BUILD, Some(product), purposes)
        .await
        .map_err(|refused| {
            CmdError::refused(format!(
                "{refused}; the CLI of {revision} was not built or read"
            ))
        })
}
