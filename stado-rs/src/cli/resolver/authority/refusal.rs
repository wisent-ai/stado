use crate::monitor::host_silence;

/// Publish one `authority_unreachable` refusal about the authority host.
///
/// The evidence belongs to the AUTHORITY, not to the machine that noticed.
/// When the Mac mini dropped off the tailnet on 2026-08-19 this read failed
/// on the laptop with "registry authority exited with ...: ssh: connect to
/// host ... Operation timed out", and that sentence was the clearest
/// statement anything in the fleet made about the Mac mini being gone. It
/// went to `~/.stado/logs/stado-resolver.err` and nowhere else. It now also
/// lands in `reader_refusals/<authority>/`, where `stado host link
/// <authority>` will find it — verbatim, because a rephrased sentence is a
/// second vocabulary for one condition and sends an operator grepping for a
/// string that exists in no source file.
///
/// Best effort and bounded by [`host_silence::report_refusal`]: this runs
/// inside a failing read and must never replace that read's own error.
pub(crate) async fn refuse_authority(target: &str, reader: &str, sentence: &str) {
    host_silence::report_refusal(
        target,
        reader,
        host_silence::REASON_AUTHORITY_UNREACHABLE,
        sentence,
    )
    .await;
}
