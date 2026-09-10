//! Judging one host's newest report against the declarations it must satisfy.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::observations::{Freshness, OBSERVED};
use crate::release_control::ProductReleasePolicy;

use super::{HostSoftware, Report, UNKNOWN};

/// The one invocation that refreshes a host's software report, spelled once so
/// the sentence that names it and the test that runs it cannot drift apart.
/// `{host}` is the registry target.
pub const REFRESH_COMMAND: &str = "stado release host-state --host {host}";

// ---------------------------------------------------------------------------
// Judging
// ---------------------------------------------------------------------------

/// The release-control product one target is supposed to be running, as a
/// concrete file on that host.
#[derive(Debug, Clone)]
pub struct ProductBinary {
    /// The program's basename, which is how the host reports it.
    pub name: String,
    /// The absolute path the release policy installs it at.
    pub path: String,
    /// `None` when the registry declares no desired release, which is a
    /// different finding from a disagreement.
    pub desired: Option<String>,
}

impl ProductBinary {
    /// The binary one release policy installs on a host, as the concrete file
    /// the host's software report names.
    ///
    /// The rollout's own artefact lives under the product install root, so it
    /// is in no `managed_versions` entry and in no `$HOME/.stado/bin` listing.
    /// A reader that did not resolve this path could compare the desired
    /// version against nothing at all — which is what `observed=unreported`
    /// was — and a writer that did not inspect it could never confirm it.
    pub fn of(policy: &ProductReleasePolicy) -> Self {
        let path = format!(
            "{}/{}",
            policy.install_root.trim_end_matches('/'),
            policy.binary.trim_start_matches('/')
        );
        Self {
            name: path
                .rsplit('/')
                .next()
                .unwrap_or(policy.binary.as_str())
                .to_string(),
            path,
            desired: policy
                .desired
                .as_ref()
                .map(|desired| desired.version.clone()),
        }
    }
}

/// What one target's software report says about the declarations it is supposed
/// to satisfy.
#[derive(Debug, Clone, Default)]
pub struct Finding {
    /// True when this target is in a state an operator has to act on.
    pub failed: bool,
    /// One sentence per disagreement, each naming the host and the exact
    /// disagreement. Empty when there is nothing to say.
    pub sentences: Vec<String>,
}

impl Finding {
    /// The one word a screen sorts and colours on. `ok` is only ever reached by a
    /// fresh report in which every declared program is accounted for.
    pub fn word(&self) -> &'static str {
        if self.failed {
            "failed"
        } else {
            "ok"
        }
    }

    /// The verdict, folded into the report it is about.
    ///
    /// One object rather than a nested one, so a consumer reads `verdict` and
    /// `findings` beside the counts they were computed from. `verdict` and not
    /// `state`: the report already carries a `state` — whether the look
    /// happened — and two different questions under one key is how a screen
    /// comes to colour "nobody looked" as "everything is wrong", or worse, the
    /// other way round.
    pub fn merge_into(&self, report: &mut Value) {
        let Some(object) = report.as_object_mut() else {
            return;
        };
        object.insert("verdict".to_string(), json!(self.word()));
        object.insert("failed".to_string(), json!(self.failed));
        object.insert("findings".to_string(), json!(self.sentences));
    }

    pub fn json(&self) -> Value {
        let mut value = json!({});
        self.merge_into(&mut value);
        value
    }

    fn fail(&mut self, sentence: String) {
        self.failed = true;
        self.sentences.push(sentence);
    }
}

/// Everything wrong with one program, in one sentence, or nothing.
///
/// One sentence per program rather than one per fault: an operator reading a gate
/// wants the row and everything the fleet has against it, and splitting
/// "unmanaged" from "wrong version" into two lines about one file makes the
/// output twice as long without adding a fact.
fn disagreement(host: &str, row: &HostSoftware, declared: Option<&str>) -> Option<String> {
    let mut faults: Vec<String> = Vec::new();
    if !row.is_release() {
        faults.push(format!(
            "its digest {} matches no release artefact Stado published, so it is {}",
            row.short_digest(),
            row.provenance
        ));
    }
    match declared {
        Some(want) if row.version == UNKNOWN => faults.push(format!(
            "it reports no version at all, so the declared {want} cannot be confirmed"
        )),
        Some(want) if want != row.version => faults.push(format!("the fleet declares {want}")),
        _ => {}
    }
    if faults.is_empty() {
        return None;
    }
    Some(format!(
        "{host} runs {} {} at {}: {}",
        row.name,
        row.version,
        row.path,
        faults.join(", and ")
    ))
}

/// Does this host's newest report account for what the fleet declares it runs?
///
/// `declared` is the host's `managed_versions`: name to exact version, the same
/// primitive `service converge` and `host release` judge against. `product` is
/// the release-control binary rolled out to this target, which is declared
/// somewhere else entirely and lives under the product's own install root, so it
/// appears in none of the `managed_versions` entries.
///
/// Every failure here is a state an operator has to act on, and every one of them
/// was previously either invisible or printed beside a zero exit.
pub fn judge(
    report: &Report,
    declared: &BTreeMap<String, String>,
    product: Option<&ProductBinary>,
) -> Finding {
    let mut finding = Finding::default();
    let host = report.host.as_str();

    // The command each sentence names is the one live read this binary still
    // has: `stado host software` refreshed this report until the host verbs
    // collapsed into the release capability on 2026-09-06, and for four days
    // afterwards `release status` kept sending operators to a verb that
    // answered `Usage: stado host <COMMAND>`. Nothing wrote a report in those
    // four days either, which is why every host read `stale`.
    match &report.freshness {
        Freshness::Never => {
            finding.fail(format!(
                "{host} has never reported what software it runs, so every version claimed for it \
                 is a declaration nothing on the host confirms: run `{}`",
                REFRESH_COMMAND.replace("{host}", host)
            ));
            return finding;
        }
        Freshness::Stale(_) => finding.fail(format!(
            "{host} last reported its software {}, past the window an observation speaks for, so \
             nothing here describes the present: run `{}`",
            report.age(),
            REFRESH_COMMAND.replace("{host}", host)
        )),
        Freshness::Fresh(_) => {}
    }
    if report.state() != OBSERVED {
        finding.fail(format!(
            "{host} could not report its software ({}): {}",
            report.state(),
            report.refusal()
        ));
        return finding;
    }

    // The registry's per-binary statement of what this host must run, checked
    // against the bytes. `service converge` makes this comparison on versions
    // alone; the digest half is what tells a delivered build apart from one
    // somebody carried over by hand at the same version number.
    for (name, want) in declared {
        match report.find(name) {
            None => finding.fail(format!(
                "{host} declares {name} {want} and its software report names no {name} program at \
                 all, so the declaration is unconfirmed on the host that carries it"
            )),
            Some(row) => {
                if let Some(sentence) = disagreement(host, row, Some(want)) {
                    finding.fail(sentence);
                }
            }
        }
    }

    // The rollout's own binary. Matched on its declared path first: the product
    // install root is where the release puts it, and a same-named program
    // elsewhere on the host is a different file.
    if let Some(product) = product {
        let found = report
            .rows
            .iter()
            .find(|row| row.path == product.path)
            .or_else(|| report.find(&product.name));
        match found {
            None => finding.fail(format!(
                "{host} reports no {} program at {}, so the desired {} is confirmed nowhere on the \
                 host it rolls out to",
                product.name,
                product.path,
                product.desired.as_deref().unwrap_or("release")
            )),
            Some(row) => {
                if let Some(sentence) = disagreement(host, row, product.desired.as_deref()) {
                    finding.fail(sentence);
                }
            }
        }
    }

    finding
}
