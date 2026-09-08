//! `stado host provenance` — what accounts for each artifact a host
//! carries.

pub(in crate::cli::host) mod report;

/// Read back both halves of the question: what the host runs, and what it can
/// account for.
///
/// Two enumerations rather than one. Listing only the manifests would answer
/// "what has been recorded", which is never the failing case -- a recorded
/// artifact is by definition one somebody bothered to record. The binaries in
/// `.stado/bin` are the population; the manifests are the coverage; the
/// difference is the finding.
///
/// `.previous` builds are skipped: they are the rollback copy of an artifact
/// already listed under its own name, and reporting a second unprovenanced row
/// for each installed program would bury the real ones.
///
/// Manifests are flattened to one line each so the two kinds of output can be
/// told apart by tag rather than by parsing position, and a host that answers
/// with unexpected noise cannot turn into a fabricated row.
const READ_PROVENANCE_BODY: &str = r#"bin="$HOME/.stado/bin"
dir="$HOME/.stado/provenance"
if [ -d "$bin" ]; then
  for program in "$bin"/*; do
    [ -f "$program" ] || continue
    case "${program##*/}" in .*|*.previous) continue ;; esac
    # A release artifact is a compiled program; a helper is a checked-in script
    # left over from the retired helper channel. Both live in this directory and
    # only the first is something a release pipeline produces, so reporting them
    # in one list buries the question being asked. control-host carries
    # dozens of helpers accumulated over months -- the channel had a writer and
    # no reaper, the same accretion that fills ~/.stado/forwards with markers
    # for services that were renamed years of incidents ago. The shebang is the
    # honest discriminator and it is readable without executing anything.
    kind=binary
    case "$(/usr/bin/head -c 2 "$program" 2>/dev/null)" in '#!') kind=script ;; esac
    # The manifest is a claim about specific bytes. Reporting its commit without
    # checking it still describes the file beside it is the same unverified
    # declaration this command exists to find: on 2026-08-12 this laptop's
    # manifest named a commit while the binary next to it had been replaced by
    # hand, and the tool repeated the manifest with a straight face.
    digest=-
    if [ "$kind" = binary ]; then
      if [ -x /usr/bin/shasum ]; then
        digest=$(/usr/bin/shasum -a 256 "$program" | /usr/bin/awk '{print $1}')
      elif command -v sha256sum >/dev/null 2>&1; then
        digest=$(sha256sum "$program" | /usr/bin/awk '{print $1}')
      fi
    fi
    printf 'STADO-ARTIFACT %s %s %s\n' "$kind" "$digest" "${program##*/}"
  done
fi
if [ -d "$dir" ]; then
  for manifest in "$dir"/*.json; do
    [ -f "$manifest" ] || continue
    printf 'STADO-MANIFEST %s\n' "$(/usr/bin/tr -d '\n\r' < "$manifest")"
  done
fi
"#;

/// One artifact a host carries, joined to whatever accounts for it.
struct CarriedArtifact {
    artifact: String,
    record: Option<crate::provenance::Provenance>,
    /// `None` is "no checkout here could answer", never "no". An operator who
    /// is told `no` walks a build back; one who is told `unknown` clones the
    /// repository first. Collapsing the two is how a fleet learns to disregard
    /// its own reports.
    reachable: Option<bool>,
    /// Does the manifest still describe the bytes beside it? `None` when there
    /// is no manifest or the host could not hash the file. A manifest naming a
    /// commit for a binary that has since been replaced is worse than no
    /// manifest: it answers the provenance question confidently and wrongly,
    /// which is exactly the failure this command was built to expose.
    describes: Option<bool>,
    age_seconds: Option<i64>,
}
