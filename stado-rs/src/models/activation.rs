//! The activation extractors the wisent-tools Python package used to ship.
//!
//! The package no longer contains any Python, so a job whose command runs one
//! of its `wisent.scripts.activations` modules can only fail on its worker,
//! after a claim, a GPU and a boot, with an import error that says nothing
//! about why. It is refused where it is submitted, and again where a slot
//! would start one already queued, naming what replaced it: Ster reads
//! activations natively (`ster extract`, `ster vector train`) and refuses a
//! run with too few pairs or activations with the counts.

pub const REMOVED_ACTIVATION_PACKAGE: &str = "wisent.scripts.activations";

/// Why a job running `command` cannot run, when it names a removed extractor.
pub fn removed_activation_command_reason(command: &str) -> Option<&'static str> {
    command.contains(REMOVED_ACTIVATION_PACKAGE).then_some(
        "refusing a wisent.scripts.activations job: wisent-tools no longer ships its Python \
         activation extractors, so the job would fail on import on its worker; read \
         activations with `ster extract` or `ster vector train`, which refuse too few pairs \
         or activations with the counts",
    )
}
