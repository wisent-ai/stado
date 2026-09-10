//! What is this host actually running, and did Stado put it there?
//!
//! On 2026-08-18 `stado release status` printed
//! `brama target=control-host desired=0.2.27 observed=unreported` and
//! exited zero. A host that had never once said what it runs was rendered
//! indistinguishable from a healthy one, in the command an operator reaches for
//! to ask exactly that. On the same day two machines were running a skarbiec
//! built on somebody's laptop — 0.2.1 here, 0.2.3 on control-host, neither
//! of them in any published release — and the pre-fix binary was stripping the
//! `brama:agent:<id>` tags off a live credential every rotation, which removed a
//! working subscription from the fleet while the credential itself stayed valid.
//! No screen in this fleet could name the program doing it, because no screen
//! knew the program existed.
//!
//! Three separate silences, one shape: the fleet stored **declarations** about
//! software and never an **observation** of it. `managed_versions` says what a
//! host must run. `release_control.desired` says what must be rolled out. A
//! service declaration names a unit and a plist. All three stay true across
//! every release that never reached the box, and none of them is about the bytes
//! on the disk.
//!
//! So this module records the other half, and records it the way
//! [`crate::observations`] records everything else that decays: as a look, taken
//! at a moment, by a named vantage, that goes stale. One row per program:
//! `{ name, path, version, sha256, provenance }`.
//!
//! `provenance` is [`RELEASE`] when those exact bytes are also a staged release
//! artefact under `$HOME/.stado/releases`, and [`UNMANAGED`] otherwise. It is
//! decided by digest and by nothing else, on the host, because a name, a version
//! string and a program's own claim about its provenance all survive one `scp`,
//! and a digest that equals the extracted member of an archive Stado verified
//! against the canonical release manifest does not.
//! [`crate::deploy::host_release`] stages every delivery under its own immutable
//! coordinate and hard-links it into place, so a program that came through the
//! sanctioned channel matches and one that did not, does not — which makes
//! `unmanaged` a finding rather than a gap in what this could measure.
//!
//! **Silence is a failure here, and that is the whole point.** A host with no
//! report, a report older than [`crate::observations::DEFAULT_TTL`], a declared
//! program that is absent, an `unmanaged` program, or a version that disagrees
//! with what the fleet declares are all failures out of [`judge`], each in one
//! sentence that names the host and the exact disagreement.
//!
//! What is deliberately *not* a failure is a program nothing declares. This
//! laptop carries eleven dated backup copies of `stado` in `$HOME/.stado/bin`,
//! none of them running, and failing forever on those is how an operator learns
//! to write `|| true` after the command — at which point the drift this exists
//! to catch stops being noticed again, exactly as
//! `service_converge::report_gate` argues. Every such program is still reported,
//! still counted and still visible in `stado release host-state`; it just does
//! not decide the gate. Accountability is resolved against the live registry on
//! every read rather than frozen into the record, for the reason
//! [`crate::binary::provenance`] does not store reachability: a declaration added an
//! hour after a report must bring that program into scope, and a stored verdict
//! would still be answering the older question.
//!
//! The report has exactly one writer, and it is the live read an operator
//! already reaches for. `stado host software` wrote it until the host verbs
//! collapsed into the release capability on 2026-09-06; that change deleted
//! the verb and kept everything the verb fed, so for four days `release
//! status` judged reports nothing could refresh and sent operators to a
//! command that no longer parsed. [`refresh`] is the writer now, and
//! `stado release host-state` calls it on every report and every apply: one
//! command reads the host, and both the drift verdict and this report come
//! out of that one visit.
//!
//! The components are the seams this file already carried: [`row`] holds the
//! one program on one host, [`report`] holds the newest report and the store it
//! is written to and read back from, [`inspect`] holds the read of the host
//! itself, and [`verdict`] holds the judgement. Every name a caller outside
//! this module uses is re-exported here, so `crate::host_software::<item>`
//! resolves exactly as before.

mod inspect;
mod report;
mod row;
mod verdict;

pub use inspect::{gather, parse};
pub use report::{load, load_in, record, record_refusal, reported_hosts, Report};
pub use row::HostSoftware;
pub use verdict::{judge, Finding, ProductBinary, REFRESH_COMMAND};

use serde_json::Value;

use crate::deploy::{DeployError, Runner};
use crate::targets::ComputeTarget;

/// Take a fresh look at TARGET and put it on file, replacing the last one.
///
/// `programs` are the paths the caller binds beyond what the host lists
/// itself: the release-control products rolled out to it, and the artefact
/// roots the drift reporter just resolved. What comes back is the report as
/// the store now holds it — read back rather than returned from the gather —
/// so a caller prints exactly what `release status` will judge next.
///
/// A read the channel refuses is recorded as a refusal and returned as one,
/// never raised past this function: the previous report must stop reading as
/// current the moment the host cannot be read, and an error here would leave
/// it on file looking fresh. Only a store that cannot be written is an error,
/// because then nothing about the look survived.
pub async fn refresh(
    target: &ComputeTarget,
    programs: &[String],
    runner: &Runner,
) -> Result<Report, DeployError> {
    match gather(target, programs, runner).await {
        Ok((rows, scripts)) => record(&target.name, &rows, scripts),
        Err(error) => record_refusal(&target.name, &error.0),
    }
    .map_err(|error| {
        DeployError(format!(
            "{}: the software report could not be written: {error}",
            target.name
        ))
    })?;
    Ok(load(&target.name))
}

/// The release-control products rolled out to HOST, as the concrete files a
/// software report about that host has to name.
///
/// A registry without release control, or one whose control block does not
/// parse, rolls nothing out, and that is an empty list rather than an error:
/// the report is still owed for everything else the host runs.
pub fn products_rolled_out_to(document: &Value, host: &str) -> Vec<ProductBinary> {
    let Ok(Some(control)) = crate::release_control::control(document) else {
        return Vec::new();
    };
    control
        .products
        .values()
        .filter(|policy| policy.targets.contains_key(host))
        .map(ProductBinary::of)
        .collect()
}

/// The bytes came out of a release Stado published and verified.
pub const RELEASE: &str = "release";
/// The bytes match no release artefact this host carries. A finding.
pub const UNMANAGED: &str = "unmanaged";
/// The reporter looked and the program would not say. Never rounded to a
/// version, and never rounded to agreement.
pub const UNKNOWN: &str = "unknown";
/// What [`report_fact`] prefixes, named once because [`reported_hosts`] reads it
/// back off the fact.
const REPORT_KIND: &str = "software-report:";

/// The canonical fact name for "what is this program on this host".
///
/// One spelling, shared by the writer and by every reader, for the reason
/// [`crate::observations::service_fact`] has one: a fact recorded under one name
/// and looked up under another is a fact with no reader.
pub fn software_fact(name: &str, host: &str) -> String {
    format!("software:{name}@{host}")
}

/// The canonical fact name for "did this host report its software at all".
///
/// A separate fact from the programs it lists, and the one that makes silence
/// legible: per-program rows can only ever say what was there, so without a row
/// for the report itself a host that never answered and a host whose programs
/// were all removed would read identically. It also bounds the report —
/// [`observations::record`](crate::observations::record) merges and never
/// deletes, so a program gone from the host would otherwise stay on file
/// forever and read as present.
pub fn report_fact(host: &str) -> String {
    format!("{REPORT_KIND}{host}")
}
