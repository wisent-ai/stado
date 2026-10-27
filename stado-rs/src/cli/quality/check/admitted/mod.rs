//! What a quality check runs that the operator's rulings govern, asked of
//! Oko's admission decision (`oko admit compile|test|build --product P
//! --purpose DEFECT`) before it runs: a lint gate compiles the tree, a test
//! gate runs its tests, and reading the command tree builds the CLI. A
//! format gate reads the committed bytes alone and asks nothing. The
//! operation that started the check passes its admission down (`allowed`).

use crate::cli::release_loop::allowed::{refusal, BUILD, COMPILE, TEST};
use crate::cli::CmdError;
use crate::release_pipeline::GateKind;

/// The action a gate of `kind` takes, when the rulings govern it.
fn action(kind: GateKind) -> Option<&'static str> {
    match kind {
        GateKind::Format => None,
        GateKind::Lint => Some(COMPILE),
        GateKind::Test => Some(TEST),
    }
}

/// Ok when gate `name` of kind `kind` may run on `revision` of `product` as
/// work serving `purposes`; else the refusal naming the ruling and the gate.
pub(super) async fn gate(
    kind: GateKind,
    name: &str,
    product: &str,
    revision: &str,
    purposes: &[&str],
) -> Result<(), CmdError> {
    let Some(action) = action(kind) else {
        return Ok(());
    };
    refusal(action, Some(product), purposes)
        .await
        .map_err(|refused| {
            CmdError::refused(format!(
                "{refused}; gate {name:?} of {revision} was not run"
            ))
        })
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
