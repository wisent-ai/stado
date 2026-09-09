//! `stado host provenance` — what accounts for each artifact a host
//! carries.

pub(in crate::cli::host) mod report;
mod trailers;

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
/// Two kinds of record account for an artefact, and both are read here. A
/// build manifest under `.stado/provenance` is written by whoever compiled the
/// bytes; a delivery receipt under
/// `.stado/releases/<binary>/<version>/<platform>/release-receipt.json` is
/// written by the release path when it installs them, and carries the same
/// four facts - commit, digest, when, by whom. Reading only the first reported
/// every pipeline-delivered binary as `unprovenanced`, which is the confident
/// wrong answer this command exists to prevent: on 2026-09-08 a leased target
/// on `charless-mac-mini` was delivered 0.16.38 through the product, kept the
/// receipt beside the staged copy, and was still reported as accounted for by
/// nothing.
///
/// Manifests and receipts are flattened to one line each so the three kinds of
/// output can be told apart by tag rather than by parsing position, and a host
/// that answers with unexpected noise cannot turn into a fabricated row.
const READ_PROVENANCE_BODY: &str = r#"bin="$HOME/.stado/bin"
dir="$HOME/.stado/provenance"
releases="$HOME/.stado/releases"
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
    # A marker is neither: the delivery path writes
    # `$HOME/.stado/bin/stado.release-version`, one line naming the version it
    # just activated, and it is not executable. Counting it as an artifact put
    # a permanently `unprovenanced` row in the table for a file the release
    # path itself created -- a row nobody can ever close, next to the rows that
    # matter.
    kind=binary
    case "$(/usr/bin/head -c 2 "$program" 2>/dev/null)" in '#!') kind=script ;; esac
    if [ "$kind" = binary ] && [ ! -x "$program" ]; then kind=marker; fi
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
if [ -d "$releases" ]; then
  for receipt in "$releases"/*/*/*/release-receipt.json; do
    [ -f "$receipt" ] || continue
    printf 'STADO-RECEIPT %s\n' "$(/usr/bin/tr -d '\n\r' < "$receipt")"
  done
fi
"#;

/// What accounts for one artefact: a build manifest, a delivery receipt, or
/// nothing.
///
/// Named in the report, because the two records answer with different
/// authority. A manifest is written by whoever compiled the bytes; a receipt
/// is written by the release path that installed them, against a digest it
/// verified on the way in. Printing one word for both would hide which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::cli::host) enum AccountedBy {
    Manifest,
    Receipt,
    Nothing,
}

impl AccountedBy {
    pub(in crate::cli::host) fn as_str(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::Receipt => "release-receipt",
            Self::Nothing => crate::provenance::UNPROVENANCED,
        }
    }
}

/// One delivery receipt as the release path writes it.
#[derive(Debug, Clone, serde::Deserialize)]
pub(in crate::cli::host) struct DeliveryReceipt {
    pub(in crate::cli::host) binary: String,
    pub(in crate::cli::host) version: String,
    pub(in crate::cli::host) platform: String,
    pub(in crate::cli::host) source_commit: String,
    /// Digest of the release archive the manifest named. Not a digest of
    /// anything the host keeps, so it cannot be matched against a file.
    pub(in crate::cli::host) sha256: String,
    /// Digest of the installed file this receipt sits beside. Absent from
    /// receipts written before the field existed, and from a tree delivery,
    /// which installs a directory.
    #[serde(default)]
    pub(in crate::cli::host) artifact_sha256: String,
    pub(in crate::cli::host) installed_at: String,
    pub(in crate::cli::host) delivered_by: String,
}

/// One artifact a host carries, joined to whatever accounts for it.
struct CarriedArtifact {
    artifact: String,
    record: Option<crate::provenance::Provenance>,
    /// The delivery receipt whose recorded artefact digest equals the
    /// installed bytes, when one exists. Digest-matched on purpose: a receipt
    /// for another version says nothing about the file in place, and reading
    /// it as provenance would be the same unverified claim a hand-edited
    /// manifest makes.
    receipt: Option<DeliveryReceipt>,
    /// `None` is "no checkout here could answer", never "no". An operator who
    /// is told `no` walks a build back; one who is told `unknown` clones the
    /// repository first. Collapsing the two is how a fleet learns to disregard
    /// its own reports.
    reachable: Option<bool>,
    /// Does the record still describe the bytes beside it? `None` when there
    /// is nothing accounting for the artefact or the host could not hash the
    /// file. A record naming a commit for a binary that has since been
    /// replaced is worse than no record: it answers the provenance question
    /// confidently and wrongly, which is exactly the failure this command was
    /// built to expose.
    describes: Option<bool>,
    age_seconds: Option<i64>,
}

impl CarriedArtifact {
    fn accounted_by(&self) -> AccountedBy {
        match (&self.record, &self.receipt) {
            (Some(_), _) => AccountedBy::Manifest,
            (None, Some(_)) => AccountedBy::Receipt,
            (None, None) => AccountedBy::Nothing,
        }
    }

    /// The commit whichever record accounts for these bytes names.
    fn commit(&self) -> String {
        match (&self.record, &self.receipt) {
            (Some(record), _) => record.commit.clone(),
            (None, Some(receipt)) => receipt.source_commit.clone(),
            (None, None) => crate::provenance::UNPROVENANCED.to_string(),
        }
    }

    /// Who produced or delivered these bytes.
    fn builder(&self) -> String {
        match (&self.record, &self.receipt) {
            (Some(record), _) => record.builder.clone(),
            (None, Some(receipt)) => receipt.delivered_by.clone(),
            (None, None) => "-".to_string(),
        }
    }

    /// The version a delivery receipt names, for the rows that have one.
    fn version(&self) -> Option<&str> {
        self.receipt
            .as_ref()
            .map(|receipt| receipt.version.as_str())
    }

    /// Is anything accounting for these bytes at all?
    fn accounted(&self) -> bool {
        self.record.is_some() || self.receipt.is_some()
    }

    /// When the record accounting for these bytes was written.
    fn stamp(&self) -> Option<String> {
        match (&self.record, &self.receipt) {
            (Some(record), _) => Some(record.at.clone()),
            (None, Some(receipt)) => Some(receipt.installed_at.clone()),
            (None, None) => None,
        }
    }
}
