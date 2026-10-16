use crate::monitor::host_silence;

/// Publish one `authority_unreachable` refusal about the authority host,
/// without the failing read standing behind its write.
///
/// The evidence belongs to the AUTHORITY, not to the machine that noticed:
/// it lands in `reader_refusals/<authority>/`, where `stado host link
/// <authority>` finds it, with the read's own sentence verbatim.
///
/// The fleet store that record goes to is served from the same authority
/// host the read just failed to reach. Awaited, every failing read then
/// waited a second time on that host before its caller saw the refusal, and
/// each one-shot command (every agent hook asks the directory) added one
/// more write to the queue in front of that host. The write is detached: a
/// one-shot command that exits first loses its copy, and the resolver on
/// the same machine, which reads the same authority, records the same
/// refusal from its own long-lived process.
pub(crate) fn refuse_authority(target: &str, reader: &'static str, sentence: &str) {
    host_silence::report_refusal_detached(
        target.to_string(),
        reader,
        host_silence::REASON_AUTHORITY_UNREACHABLE,
        sentence.to_string(),
    );
}
