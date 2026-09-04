//! The release agent's unit-image revisit pass: put ONE declared launchd unit
//! per reconcile invocation back on the file it declares, and record what
//! happened.
//!
//! `registry doctor` sees a unit whose live process executes a replaced or
//! unlinked image (#336) and `stado service refresh-image` repairs one named
//! unit on demand (#344). Neither revisits a unit nobody typed a command for:
//! `self_update::recycle_replaced_units` cycles units only inside the
//! invocation that replaced their bytes, so one it misses stays missed —
//! `com.wisent.compute.disk-cleanup.disk-cleanup` journalled `policy:ValueError`
//! 8,348 times over thirteen days that way, and an unrelated restart ended it.
//! The installed binary moved from 0.13.50 to 0.14.8 inside one day, so the
//! condition regenerates faster than a per-unit manual verb clears it.
//!
//! Four bounds, each enforced in one named place:
//!
//! - **The `release_unit_image_revisit` registry block names exact labels for
//!   one host and is absent by default.** Per product AND per target, because
//!   a label is a fact about one machine: the same product's Linux target runs
//!   different units under different names, and a product-level list would
//!   have authorised one platform's labels on every platform. It is a
//!   TOP-LEVEL, unmodelled key rather than a `release_control` field so that
//!   older builds preserve and ignore it instead of refusing the whole
//!   document — see [`REVISIT_POLICY_KEY`]. [`policy`] returns `None` for
//!   today's absent key before [`host_scope`] is called. For a present block,
//!   `host_scope` answers `None` when no product names a label for this target.
//! - **One unit per reconcile invocation.** One scheduled tick is one
//!   invocation; [`revisit_plan`] picks one and records the rest as
//!   [`RevisitSkip::OneUnitPerTick`].
//! - **Never a unit that recycles itself.**
//!   `self_update::defers_to_release_handshake` is reused, not re-derived.
//! - **The identity is read again afterwards.** #344's [`refresh_outcome`]
//!   decides, and the attempt is recorded so it is not tried again on the same
//!   pair of identities.
//!
//! The contract for one host — which state directory holds the ledger, and
//! which product authorises which label on this target — is computed from
//! EVERY product in the policy before `--product` is applied, so concurrent
//! product-scoped agents share one lock and cannot both spend a restart on the
//! same unchanged identity pair.
//!
//! A non-blocking host lock covers observe → record → kickstart → settle →
//! record. What it prevents is OVERLAP: two reconciles running concurrently
//! would each observe the same stale unit against the same unchanged identity
//! pair and each spend a restart on it, neither having seen the other's
//! ledger write. It is not a rate limit and defines no time window.
//! Sequential invocations are separate ticks, and each may act on one unit —
//! a different one, because the unit a tick handled is afterwards either on
//! its declared file or barred by its own record.
//!
//! **The attempt is written before the restart, not after.** A record written
//! only once the outcome is known is lost by any crash or write failure in
//! between, and the next tick then kickstarts the same unit again — the hot
//! loop every other bound here exists to prevent, reappearing exactly when the
//! host is already unhealthy. So an [`AttemptOutcome::Attempting`] record is
//! committed first and the side effect is refused if that write fails; the
//! observed or refused result then replaces it. A record left at `Attempting`
//! is a recorded INTENT with no result beside it: the pass stopped somewhere
//! between committing that intent and writing down what happened, so whether
//! `launchctl` was ever invoked is unknown. It bars the same identity pair for
//! exactly that reason, and says so on the `registry doctor` row.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::service_refresh_image::{refresh_outcome, settle, RefreshOutcome};
use crate::deploy::service::{self, ImageIdentity, ImageState, StaleUnitImage, UnitImageScan};
use crate::self_update::defers_to_release_handshake;

pub(crate) const REVISIT_SCHEMA: u32 = 1;

/// The host ledger and its lock, inside the release `state_dir`.
///
/// The `@` is load-bearing: this directory also holds `<product>.json` and
/// `<product>-proxy.json`, and `release_control::identifier` admits no `@`, so
/// no product coordinate can ever name the same file.
const LEDGER_FILE: &str = "@unit-images.revisit.json";
const LOCK_STEM: &str = "@unit-images.revisit";

/// One executable file as the ledger stores it: the pair the kernel answers
/// with. Sizes, paths and link counts are deliberately absent — they move
/// without the file moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

impl FileIdentity {
    fn of(image: &ImageIdentity) -> Self {
        Self {
            device: image.device,
            inode: image.inode,
        }
    }

    fn is(self, image: &ImageIdentity) -> bool {
        self.device == image.device && self.inode == image.inode
    }
}

/// What one attempt achieved.
///
/// #344's [`RefreshOutcome`] describes what a SECOND READ found, and all five
/// of its results presuppose that read. Two states of this pass have no such
/// read and must not borrow a word that claims one:
///
/// - a `kickstart` launchd refused, where recording `NotRunning` would assert
///   that nothing executes the unit's argument vector — never observed, and
///   usually false because the old process is still running the old image;
/// - an intent committed before the restart was issued whose result was never
///   written, which is what a crash anywhere in that window leaves behind. It
///   says a restart may or may not have been invoked, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptOutcome {
    /// Committed BEFORE the restart is issued, and replaced by the real result
    /// on the same tick. Still present means the pass stopped between
    /// committing the intent and writing a result, so whether
    /// `launchctl kickstart` ran at all is unknown — which is exactly what
    /// this word must be read as, and no more.
    Attempting,
    /// `launchctl kickstart` did not succeed. Nothing was read afterwards and
    /// nothing is claimed about the process.
    RestartRefused,
    /// The restart was issued and the identity was read again.
    Observed(RefreshOutcome),
}

impl AttemptOutcome {
    /// The one word a report and the ledger name this by.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Attempting => "Attempting",
            Self::RestartRefused => "RestartRefused",
            Self::Observed(outcome) => outcome.word(),
        }
    }

    /// Decode one ledger word through the same enum values [`Self::word`]
    /// encodes.
    ///
    /// The strings live only in `word`; every candidate here is a typed
    /// outcome rather than a second list of accepted spellings.
    fn parse(word: &str) -> Option<Self> {
        [
            Self::Attempting,
            Self::RestartRefused,
            Self::Observed(RefreshOutcome::OnDeclaredFile),
            Self::Observed(RefreshOutcome::NotRunning),
            Self::Observed(RefreshOutcome::Unread),
            Self::Observed(RefreshOutcome::Unchanged),
            Self::Observed(RefreshOutcome::StillWrong),
        ]
        .into_iter()
        .find(|outcome| outcome.word() == word)
    }
}

/// One attempt this host has recorded against one unit, and what came of it.
///
/// An attempt is recorded when the pass commits its intent, before any
/// restart is issued, so a row here says a restart was authorised and aimed —
/// not that `launchctl` ran. Only an `outcome` of
/// [`AttemptOutcome::Observed`] establishes that it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisitAttempt {
    /// The image the unit was executing when this attempt was recorded.
    pub was_running: FileIdentity,
    /// The file it declared then — what the attempt aimed it at.
    pub declared: FileIdentity,
    /// [`AttemptOutcome::word`].
    pub outcome: String,
    pub attempted_at: String,
    /// The launchd service target that answered, the refusal, or what the
    /// pass was about to do.
    pub service: String,
}

impl RevisitAttempt {
    /// Whether this record still bars another attempt.
    ///
    /// **How the state expires.** An attempt that did not land bars this unit
    /// only while BOTH identities are the ones it was made against, because
    /// that pair is the whole content of what it established: kickstarting
    /// this unit while it runs `was_running` and declares `declared` did not
    /// move it. Either side changing is a situation nothing has been tried in
    /// — the declared file replaced again (the common case at 0.13.50 to
    /// 0.14.8 in a day), or something else cycling the unit onto a third
    /// image, which is how `com.wisent.compute.agent.lukasz-macbook` healed
    /// itself. A wall clock is worse in both directions: too short
    /// re-kickstarts a unit launchd will not move, too long holds off a unit
    /// whose file changed an hour ago. The identity pair is not a proxy for
    /// "has anything changed" — it is that question.
    ///
    /// `Attempting` bars for the same reason and deliberately errs toward not
    /// acting. It says a restart was recorded as INTENDED against this pair
    /// and that no result was written — so whether launchd was ever asked is
    /// itself unknown. Issuing another restart is the one move that cannot be
    /// justified from that, because it is the move whose effect the record
    /// cannot rule out having already had.
    pub(crate) fn bars(&self, running: &ImageIdentity, declared: &ImageIdentity) -> bool {
        self.outcome != RefreshOutcome::OnDeclaredFile.word()
            && self.was_running.is(running)
            && self.declared.is(declared)
    }

    /// The clause `registry doctor` appends to the row that told an operator to
    /// restart this unit by hand.
    ///
    /// Outcome-specific, because one sentence cannot be true of all three
    /// shapes. Claiming "restarted and read the identity again" over a refusal
    /// or a lost result would be this feature reintroducing the defect it
    /// exists to remove, on the row an operator reads.
    pub(crate) fn clause(&self) -> String {
        let tail = "It will not be attempted again until the running image or the declared file \
                    changes";
        if self.outcome == AttemptOutcome::Attempting.word() {
            return format!(
                ". The release agent recorded its INTENT to restart this unit at {} and never \
                 recorded a result, so it stopped somewhere between committing that intent and \
                 writing down what happened. Whether `launchctl kickstart` was invoked at all is \
                 unknown: the intent is written first precisely so that a lost result cannot be \
                 mistaken for a restart that never happened, and it cannot say which of the two \
                 this was. {tail}",
                self.attempted_at
            );
        }
        if self.outcome == AttemptOutcome::RestartRefused.word() {
            return format!(
                ". The release agent tried to restart this unit at {} and launchd refused ({}); \
                 the identity was NOT re-read, so this row still describes the process that was \
                 running before. {tail}",
                self.attempted_at, self.service
            );
        }
        format!(
            ". The release agent restarted this unit at {} ({}) and read the identity again: {}. \
             {tail}",
            self.attempted_at, self.service, self.outcome
        )
    }
}

/// Every restart this host has spent on unit images, keyed by launchd label.
///
/// One document per host, never per product: which image a pid executes is a
/// property of the machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisitLedger {
    pub schema_version: u32,
    pub host: String,
    #[serde(default)]
    pub attempts: BTreeMap<String, RevisitAttempt>,
}

impl RevisitLedger {
    fn new(host: &str) -> Self {
        Self {
            schema_version: REVISIT_SCHEMA,
            host: host.to_string(),
            attempts: BTreeMap::new(),
        }
    }

    /// The attempt that bars `unit`, if one does.
    fn barring(
        &self,
        unit: &str,
        running: &ImageIdentity,
        declared: &ImageIdentity,
    ) -> Option<&RevisitAttempt> {
        self.attempts
            .get(unit)
            .filter(|attempt| attempt.bars(running, declared))
    }
}

fn ledger_path(state_dir: &str) -> PathBuf {
    Path::new(state_dir).join(LEDGER_FILE)
}

/// Read the host ledger, or start an empty one.
///
/// An absent file is an empty ledger. A file that cannot be parsed, belongs to
/// another host or schema, or carries an outcome outside
/// [`AttemptOutcome`]'s closed vocabulary is an error and NOT an empty ledger:
/// reading a ledger as empty because it could not be understood is how a
/// bounded remedy becomes an unbounded one.
fn load_ledger(state_dir: &str, host: &str) -> Result<RevisitLedger, String> {
    let path = ledger_path(state_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RevisitLedger::new(host))
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let ledger: RevisitLedger = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not a revisit ledger: {error}", path.display()))?;
    if ledger.schema_version != REVISIT_SCHEMA {
        return Err(format!(
            "{} carries schema {} and this build reads {REVISIT_SCHEMA}",
            path.display(),
            ledger.schema_version
        ));
    }
    if ledger.host != host {
        return Err(format!(
            "{} belongs to {} and this machine is {host}",
            path.display(),
            ledger.host
        ));
    }
    for (unit, attempt) in &ledger.attempts {
        if AttemptOutcome::parse(&attempt.outcome).is_none() {
            return Err(format!(
                "{} carries unknown outcome {:?} for unit {:?}",
                path.display(),
                attempt.outcome,
                unit
            ));
        }
    }
    Ok(ledger)
}

/// Durably commit the ledger through the release agent's one JSON writer:
/// same-directory unique staging, `create_new`, `write_all`, `sync_all`, then
/// rename. In particular, the `Attempting` safety boundary is not considered
/// written until those bytes are synced and committed.
fn save_ledger(state_dir: &str, ledger: &RevisitLedger) -> Result<(), String> {
    crate::release_agent::atomic_json(&ledger_path(state_dir), ledger)
}

/// The registry key this policy lives under.
///
/// **Top-level and unmodelled, deliberately.** [`crate::release_control`]'s
/// structs all carry `#[serde(deny_unknown_fields)]`, so a document holding a
/// key they do not model is refused OUTRIGHT by every build that predates it
/// — not ignored. Instance 25 in `docs/checks-that-measure-nothing.md` is
/// what that costs: `readiness_path` went from forbidden to required with no
/// version where both held, so on 2026-09-01 no single document satisfied the
/// fleet and the mini's queue agent resolved no policy at all. Declaring this
/// inside `release_control` would repeat it exactly — the first host to
/// receive the document would be the first host to stop reading the registry.
///
/// A top-level key is not modelled by [`crate::targets::Registry`], so it
/// rides in `Registry::extra`, which round-trips verbatim through every read
/// and write. Old builds preserve it and ignore it; this build reads it. That
/// is why there is no `ComputeTarget` field and no declaration-catalog entry
/// for it either: both are modelled surfaces, and adding to them is the same
/// trap in another costume.
pub(crate) const REVISIT_POLICY_KEY: &str = "release_unit_image_revisit";

/// `{schema_version, targets: {<host>: {state_dir, products: {<product>: [labels]}}}}`.
///
/// The typed parser denies unknown fields even though the key itself is
/// unmodelled: a document is welcome to carry keys this build does not know,
/// but a `release_unit_image_revisit` block with a misspelled field inside it
/// is a policy whose author expected something this build will not do, and
/// silently authorising the part it understood is how a restart nobody asked
/// for gets issued.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisitPolicy {
    pub schema_version: u32,
    pub targets: BTreeMap<String, RevisitTargetPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisitTargetPolicy {
    /// Where this host keeps its one revisit ledger and its lock.
    ///
    /// Declared here, and NOT derived from a `release_control` target policy.
    /// Deriving it would have required every authorised product to appear in
    /// `release_control` and declare this host — and the units that motivate
    /// this whole feature fail that test. `com.wisent.compute.disk-cleanup.disk-cleanup`
    /// and `com.wisent.stado-resolver` belong to the Stado release itself,
    /// which has no blue-green rollout policy; `com.wisent.transcript-lake-stream`
    /// is a real product that Stado's release-control catalogue does not carry
    /// at all. A derivation that excluded all three motivating owners would be
    /// a feature that cannot be turned on for anything it was built for.
    ///
    /// So the policy is the authorization, and it carries its own directory:
    /// one per host, so one ledger and one lock hold the
    /// one-attempt-per-unit bound.
    pub state_dir: String,
    /// Product name to the exact launchd labels it authorises on this host.
    ///
    /// The product name is a label for who consented, not a lookup into
    /// `release_control`. These units are otherwise unowned, which is exactly
    /// why an explicit authorization is what makes them touchable.
    pub products: BTreeMap<String, Vec<String>>,
}

/// The policy block, or `None` when the document carries none.
///
/// Absent means off, and that is the whole default: no registry in this fleet
/// carries the key, so every reader of this function returns before it looks
/// at a process table, a unit file, a lock or a disk.
///
/// A block that is present and will not parse is an `Err` and never a `None`.
/// Reading a malformed policy as "nothing authorised" would be the same defect
/// this module exists to remove, one level up: an unread declaration rendered
/// as a clean one.
pub(crate) fn policy(document: &Value) -> Result<Option<RevisitPolicy>, String> {
    let Some(block) = document.get(REVISIT_POLICY_KEY) else {
        return Ok(None);
    };
    <RevisitPolicy as Deserialize>::deserialize(block)
        .map(Some)
        .map_err(|error| format!("registry.{REVISIT_POLICY_KEY} is not readable: {error}"))
}

/// Every product a shipped declaration or an adopted service says owns
/// `label` on `target_name`.
///
/// These are positive witnesses, not prerequisites. An explicit revisit
/// policy remains sufficient when neither catalogue carries ownership, but it
/// may not contradict either catalogue. All witnesses are retained so a
/// contradictory registry cannot be hidden by whichever source happened to be
/// searched first.
fn declared_owners(
    document: &Value,
    target_name: &str,
    label: &str,
) -> Result<std::collections::BTreeSet<String>, String> {
    let mut owners = std::collections::BTreeSet::new();
    let products = crate::deploy::products::declared()
        .map_err(|error| format!("cannot read the shipped product declarations: {error}"))?;
    for product in products {
        if product
            .units
            .iter()
            .any(|unit| unit.label_for(target_name) == label)
        {
            owners.insert(product.name.clone());
        }
    }
    let services = document
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|target| target.get("name").and_then(Value::as_str) == Some(target_name))
        .filter_map(|target| target.get("services").and_then(Value::as_array))
        .flatten();
    for service in services {
        // Match `service::declared_services`: a declared-only record is a
        // pre-adoption placeholder, not a managed service and therefore not a
        // positive runtime ownership witness even when it carries onboarding.
        if service.get("declared_only").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        // Exact unit identity only. `ManagedService::matches` also accepts the
        // logical service name, which is deliberately broader than ownership.
        let named = |key: &str| {
            service
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        };
        if named("label").or_else(|| named("unit")) != Some(label) {
            continue;
        }
        if let Some(owner) = service
            .get("onboarding")
            .and_then(|value| value.get("product_id"))
            .and_then(Value::as_str)
        {
            owners.insert(owner.to_string());
        }
    }
    Ok(owners)
}

/// Refuse a `release_unit_image_revisit` block that cannot mean what it says.
///
/// Wired into [`crate::targets::validate_registry_body`] beside the other
/// extension validators, so an operator learns at the write and a build that
/// disagrees with the document reports it through the existing
/// `build-refuses-registry` finding rather than acting on half of it.
pub(crate) fn validate_registry_contract(document: &Value) -> Result<(), String> {
    let Some(policy) = policy(document)? else {
        return Ok(());
    };
    let location = format!("registry.{REVISIT_POLICY_KEY}");
    if policy.schema_version != 1 {
        return Err(format!("{location}.schema_version must be 1"));
    }
    let platforms: BTreeMap<&str, &str> = document
        .get("targets")
        .and_then(Value::as_array)
        .map(|targets| {
            targets
                .iter()
                .filter_map(|target| {
                    Some((
                        target.get("name")?.as_str()?,
                        target
                            .get("release_platform")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    for (target_name, target_policy) in &policy.targets {
        let at = format!("{location}.targets.{target_name}");
        let Some(platform) = platforms.get(target_name.as_str()) else {
            return Err(format!("{at}: unknown target"));
        };
        // launchd-only. The restart runs `launchctl kickstart -k`, which
        // exists only on Darwin, so every authorised label on a Linux target
        // would fail and record `RestartRefused` against the identity pair.
        // That record bars the pair, so it is not a hot loop — but it is not a
        // resting state either: each replacement of the declared file changes
        // the identity, expires the row, and buys one more `launchctl` call
        // that cannot succeed for the same reason as the last. The host spends
        // one futile restart per release indefinitely and records each as a
        // repair considered. Matched on `darwin-` and not `darwin` because
        // `release_platform` is an `<os>-<arch>` coordinate.
        if !platform.starts_with("darwin-") {
            return Err(format!(
                "{at}: release_platform is {platform:?}, and unit-image revisit restarts through \
                 launchctl, which only Darwin has"
            ));
        }
        if !crate::release_control::safe_absolute(&target_policy.state_dir) {
            return Err(format!(
                "{at}.state_dir must be an absolute path with no '..' component"
            ));
        }
        // One `(target, label)` has one owning product or no owner: two
        // claimants means no single product authorised the restart. Scoped to
        // the target, because two hosts may legitimately run units of the same
        // name — that is what a launchd label is.
        let mut owners: BTreeMap<&str, &str> = BTreeMap::new();
        for (product, units) in &target_policy.products {
            let at = format!("{at}.products.{product}");
            if !crate::targets::is_product_identifier(product) {
                return Err(format!("{at}: is not a canonical product name"));
            }
            if units.is_empty() {
                return Err(format!(
                    "{at}: declares no units; omit the product rather than authorising an empty \
                     list, so that off is spelled one way"
                ));
            }
            for unit in units {
                // The same shape every other canonical name in this document
                // is held to, plus the dot every launchd label carries.
                if !crate::release_control::identifier(unit) || !unit.contains('.') {
                    return Err(format!("{at}: {unit} is not a launchd label"));
                }
                if let Some(owner) = owners.insert(unit.as_str(), product.as_str()) {
                    return Err(format!(
                        "{at}: {unit} on {target_name} is already authorised by {owner}; a unit \
                         on one host has one authorising product"
                    ));
                }
                // Where ownership IS written down, the policy may not
                // contradict it. Silence is permission, but every positive
                // witness must agree; retaining all witnesses also exposes a
                // shipped/onboarding contradiction instead of letting search
                // order choose an owner.
                for owner in declared_owners(document, target_name, unit)? {
                    if owner != *product {
                        return Err(format!(
                            "{at}: {unit} is declared as owned by {owner}, so {product} cannot \
                             authorise restarting it"
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Every launchd label the policy authorises on ONE target, and the product
/// that claimed it first.
///
/// Answers a different question from [`host_scope`]: what did the document ASK
/// FOR here. `registry doctor` needs that even when the contract does not
/// resolve, so it can say on the row that the agent will not act and why.
fn declared_units(policy: &RevisitPolicy, target_name: &str) -> BTreeMap<String, String> {
    let mut declared = BTreeMap::new();
    let Some(target) = policy.targets.get(target_name) else {
        return declared;
    };
    for (product, units) in &target.products {
        for unit in units {
            declared
                .entry(unit.clone())
                .or_insert_with(|| product.clone());
        }
    }
    declared
}

/// The host-wide revisit contract: where the ledger lives, and which product
/// authorises each launchd label on this target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisitScope {
    pub state_dir: String,
    /// launchd label to the product that authorised it, for this target only.
    pub units: BTreeMap<String, String>,
}

impl RevisitScope {
    /// The labels this process may act on, once `--product` is applied.
    ///
    /// A filtered invocation may act ONLY on that exact policy product; an
    /// unfiltered one may act on any of them. The filter narrows what is
    /// ACTED ON and never what the contract is computed from: the ledger path
    /// and the ownership map are host-wide, so a `--product brama` agent and a
    /// `--product skarbiec` agent share one ledger and one lock.
    fn owned(&self, product_filter: Option<&str>) -> BTreeMap<String, String> {
        self.units
            .iter()
            .filter(|(_, product)| product_filter.is_none_or(|want| want == product.as_str()))
            .map(|(unit, product)| (unit.clone(), product.clone()))
            .collect()
    }
}

/// The contract for one target, computed from EVERY product in the policy
/// regardless of `--product`, or `None` when nothing on this target is
/// authorised.
///
/// Every refusal here is one [`validate_registry_contract`] already made at
/// the document boundary. It is held again at the point of use because a
/// document written by a looser build can still arrive, and acting on half of
/// an unresolvable policy is worse than reporting it.
pub(crate) fn host_scope(
    policy: &RevisitPolicy,
    target_name: &str,
) -> Result<Option<RevisitScope>, String> {
    let Some(target_policy) = policy.targets.get(target_name) else {
        return Ok(None);
    };
    if target_policy.products.is_empty() {
        return Ok(None);
    }
    let mut units: BTreeMap<String, String> = BTreeMap::new();
    for (product, labels) in &target_policy.products {
        for unit in labels {
            if let Some(owner) = units.insert(unit.clone(), product.clone()) {
                return Err(format!(
                    "({target_name}, {unit}) is authorised by both {owner} and {product}; a unit \
                     on one host has one authorising product"
                ));
            }
        }
    }
    Ok(Some(RevisitScope {
        state_dir: target_policy.state_dir.clone(),
        units,
    }))
}

/// The one unit this tick will restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisitPick {
    pub unit: String,
    pub unit_path: String,
    pub product: String,
    pub pid: Option<u32>,
    /// `registry doctor`'s kind for this row.
    pub kind: &'static str,
    pub running: ImageIdentity,
    pub declared: ImageIdentity,
}

/// Why an authorised label was not the unit restarted this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RevisitSkip {
    /// Its argv carries the `agent` subcommand, so it recycles itself.
    DefersToReleaseHandshake,
    /// An attempt was already spent on this exact pair of identities.
    Attempted(RevisitAttempt),
    /// Something about this unit could not be read, so no repair can be
    /// claimed for it — `registry doctor`'s `unread-unit-image`.
    Unread { subject: String, reason: String },
    /// Eligible, and this tick already picked one.
    OneUnitPerTick,
    /// The label is authorised but [`service::observe_unit_image_scan`]
    /// returned no row for it.
    ///
    /// Reported rather than passed over in silence. Without it a mistyped
    /// label, or one naming a unit this host never installed, produced a tick
    /// line reading `unit=- outcome=none left=-` — indistinguishable from a
    /// host where every authorised unit is healthy, which is the
    /// declaration-versus-reality mismatch this whole feature exists to stop
    /// hiding.
    ///
    /// Derived from the observation pass rather than from a second parser of
    /// launchd's directories: `observe_unit_image_scan` already unions the
    /// registry's declared services with the three unit directories, so a
    /// label absent from its output is a label absent from both, and asking
    /// again with different code could only produce a second answer to one
    /// question.
    NotObserved,
}

impl RevisitSkip {
    fn sentence(&self) -> String {
        match self {
            Self::DefersToReleaseHandshake => {
                "defers to the installed-release handshake, so it recycles itself".to_string()
            }
            Self::Attempted(attempt) => format!(
                "already attempted at {} with outcome {}, and neither identity has changed since",
                attempt.attempted_at, attempt.outcome
            ),
            Self::Unread { subject, reason } => {
                format!("{subject} could not be read: {reason}")
            }
            Self::OneUnitPerTick => "eligible, deferred to a later tick".to_string(),
            Self::NotObserved => "authorised in release_unit_image_revisit but the image pass \
                                  returned no \
                                  observation for it: either no unit file in launchd's \
                                  directories carries that label — a declaration that names \
                                  nothing — or the unit is loaded and not running, and a job \
                                  that is not running holds no image"
                .to_string(),
        }
    }

    /// Whether this skip is a settled condition already written down
    /// elsewhere, so repeating it once per tick would add nothing.
    ///
    /// Only [`Self::Attempted`] qualifies. It is recorded in the ledger and
    /// annotated onto the unit's `registry doctor` row by
    /// [`RevisitAnnotations::clause`], and it reads identically on every
    /// future tick until an identity changes — at which point the unit
    /// becomes eligible again and the tick speaks. A log line per tick for a
    /// fact that is durable, dated and already reported is how a legible
    /// remedy turns into noise operators filter out.
    ///
    /// Everything else stays legible. [`Self::Unread`],
    /// [`Self::NotObserved`] and [`Self::DefersToReleaseHandshake`] are all
    /// declaration-versus-reality mismatches an operator has to resolve —
    /// authorising a label that names nothing, or one the pass may never
    /// touch, is a configuration error that should keep saying so — and
    /// [`Self::OneUnitPerTick`] is work genuinely queued for the next tick.
    fn is_settled(&self) -> bool {
        matches!(self, Self::Attempted(_))
    }
}

/// What one tick will and will not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisitPlan {
    pub pick: Option<RevisitPick>,
    pub skipped: Vec<(String, RevisitSkip)>,
}

/// Choose the one unit to restart from the labels `owned` authorises, and
/// record why every other authorised one was left.
///
/// Every authorised label is accounted for, one way or another: picked, or
/// carrying a [`RevisitSkip`] that says why not, including
/// [`RevisitSkip::NotObserved`] for a label the observation pass returned no
/// row for. A unit no policy names for this target is not considered and
/// produces no skip: it is not this feature's business, and that is the bound
/// that keeps an opted-in Brama from restarting the janitor.
///
/// **Decided entirely from `observations`.** The self-recycling exclusion
/// reads the SUBCOMMAND, not the program, because every stado unit on a host
/// runs the same binary — and it reads it from [`UnitImageScan::arguments`],
/// the vector the same scan matched its process on. Recovering the argv with
/// a second plist read would decide whether a unit may be touched from a
/// different moment than the one that produced the pid and image being acted
/// on; a replacement landing in between is exactly the window this whole
/// module exists because of. There is no second `ps` scan and no second plist
/// read anywhere in this function.
///
/// Observations arrive sorted by label, so the pick is deterministic and
/// nothing starves: after this tick the picked unit is either on its declared
/// file or barred by a ledger entry.
pub(crate) fn revisit_plan(
    observations: &[UnitImageScan],
    owned: &BTreeMap<String, String>,
    ledger: &RevisitLedger,
) -> RevisitPlan {
    // The host-wide unread row: `observe_unit_image_scan` could not measure
    // machine at all — no process table, no HOME, or no readable text
    // mappings — and reports one row with an empty unit rather than a silence.
    // Every authorised label is therefore unmeasured, and calling them
    // `NotObserved` would say the labels name nothing when the truth is that
    // nothing was looked at. Fail closed, with that row's own subject and
    // reason, from the observations already in hand.
    if let Some(unread) = observations
        .iter()
        .find_map(|scan| match &scan.observation.state {
            Some(ImageState::Unread { subject, reason }) if scan.observation.unit.is_empty() => {
                Some((subject.clone(), reason.clone()))
            }
            _ => None,
        })
    {
        let (subject, reason) = unread;
        return RevisitPlan {
            pick: None,
            skipped: owned
                .keys()
                .map(|unit| {
                    (
                        unit.clone(),
                        RevisitSkip::Unread {
                            subject: subject.clone(),
                            reason: reason.clone(),
                        },
                    )
                })
                .collect(),
        };
    }
    let mut plan = RevisitPlan {
        pick: None,
        skipped: Vec::new(),
    };
    for scan in observations {
        let row = &scan.observation;
        let Some(product) = owned.get(&row.unit) else {
            continue;
        };
        let (running, declared) = match &row.state {
            None => continue,
            Some(ImageState::Unread { subject, reason }) => {
                plan.skipped.push((
                    row.unit.clone(),
                    RevisitSkip::Unread {
                        subject: subject.clone(),
                        reason: reason.clone(),
                    },
                ));
                continue;
            }
            Some(ImageState::Unlinked { running, installed })
            | Some(ImageState::Replaced { running, installed }) => (running, installed),
        };
        // The argv this scan matched its process on, carried beside the
        // stable public observation. Re-reading the plist here would decide
        // the exclusion from a different moment than the pid and image it is
        // being applied to.
        if scan.arguments.is_empty() {
            plan.skipped.push((
                row.unit.clone(),
                RevisitSkip::Unread {
                    subject: format!("{}'s argument vector", row.unit),
                    reason: format!(
                        "the observation for {} carried no ProgramArguments, so whether this \
                         unit recycles itself could not be decided, and an exclusion that cannot \
                         be evaluated is not treated as passed",
                        row.unit_path
                    ),
                },
            ));
            continue;
        }
        if defers_to_release_handshake(&scan.arguments) {
            plan.skipped
                .push((row.unit.clone(), RevisitSkip::DefersToReleaseHandshake));
            continue;
        }
        if let Some(attempt) = ledger.barring(&row.unit, running, declared) {
            plan.skipped
                .push((row.unit.clone(), RevisitSkip::Attempted(attempt.clone())));
            continue;
        }
        if plan.pick.is_some() {
            plan.skipped
                .push((row.unit.clone(), RevisitSkip::OneUnitPerTick));
            continue;
        }
        plan.pick = Some(RevisitPick {
            unit: row.unit.clone(),
            unit_path: row.unit_path.clone(),
            product: product.clone(),
            pid: row.pid,
            kind: "stale-unit-image",
            running: running.clone(),
            declared: declared.clone(),
        });
    }
    // Every authorised label the observation pass returned no row for. The
    // host WAS measured — the host-wide unread row is handled above and
    // returns early — so this really is a label naming nothing here, and a
    // declaration that names nothing must not look like a host with nothing
    // to do. Appended after the rows so the tick line reads in observation
    // order first.
    for unit in owned.keys().filter(|unit| {
        !observations
            .iter()
            .any(|scan| &scan.observation.unit == *unit)
    }) {
        plan.skipped.push((unit.clone(), RevisitSkip::NotObserved));
    }
    plan
}

/// One tick's account of itself, in `registry doctor`'s kinds and #344's
/// outcome words. No new severity vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisitReport {
    pub host: String,
    pub acted: Option<(RevisitPick, AttemptOutcome)>,
    pub skipped: Vec<(String, RevisitSkip)>,
    /// Set when another process held the host lock, so the tick did nothing.
    pub busy: bool,
}

impl RevisitReport {
    /// The one line the agent writes per tick.
    pub(crate) fn line(&self) -> String {
        if self.busy {
            return format!(
                "stado release agent unit-image revisit host={} skipped: another reconcile holds \
                 the host revisit lock, so no unit was observed or restarted",
                self.host
            );
        }
        let acted = self.acted.as_ref().map_or_else(
            || "unit=- outcome=none".to_string(),
            |(pick, outcome)| {
                format!(
                    "unit={} product={} kind={} outcome={} running={} declared={}",
                    pick.unit,
                    pick.product,
                    pick.kind,
                    outcome.word(),
                    pick.running.describe(),
                    pick.declared.describe()
                )
            },
        );
        let skipped = self
            .skipped
            .iter()
            .map(|(unit, skip)| format!("{unit}: {}", skip.sentence()))
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "stado release agent unit-image revisit host={} {acted} left={}",
            self.host,
            if skipped.is_empty() {
                "-".to_string()
            } else {
                skipped
            }
        )
    }
}

/// Record one attempt against `unit` and commit the ledger.
fn record(
    state_dir: &str,
    ledger: &mut RevisitLedger,
    pick: &RevisitPick,
    outcome: AttemptOutcome,
    attempted_at: &str,
    service: String,
) -> Result<(), String> {
    ledger.attempts.insert(
        pick.unit.clone(),
        RevisitAttempt {
            was_running: FileIdentity::of(&pick.running),
            declared: FileIdentity::of(&pick.declared),
            outcome: outcome.word().to_string(),
            attempted_at: attempted_at.to_string(),
            service,
        },
    );
    save_ledger(state_dir, ledger)
}

/// Restart at most one authorised stale unit on this host, verify the
/// identity, and record the result.
///
/// The revisit branch in `reconcile_once` receives `Ok(None)` when [`policy`]
/// finds no `release_unit_image_revisit` key; it never calls this function or
/// [`host_scope`], so the feature adds no process-table, unit-file, lock or
/// ledger access.
///
/// `document` is the one the caller already resolved; reading the registry
/// again would be a behavioural change on a fleet that opted into nothing.
pub(crate) async fn revisit_once(
    document: &Value,
    policy: &RevisitPolicy,
    target_name: &str,
    product_filter: Option<&str>,
) -> Result<Option<RevisitReport>, String> {
    let Some(scope) = host_scope(policy, target_name)? else {
        return Ok(None);
    };
    let owned = scope.owned(product_filter);
    if owned.is_empty() {
        return Ok(None);
    }
    let registry = crate::targets::load_registry_from_str(&document.to_string())
        .map_err(|error| format!("cannot read the registry targets: {error}"))?;
    let target = registry.lookup(target_name).ok_or_else(|| {
        format!(
            "registry.{REVISIT_POLICY_KEY} authorises units on {target_name}, which names no \
             target"
        )
    })?;
    // Which image a pid executes is answerable only on the machine holding it
    // — the whole reason `observe_unit_images` takes `local_units`. `--target`
    // is an operator-supplied string, so this machine's own target is resolved
    // from its hostname the way `service refresh-image` does. Without it a
    // `--target charless-mac-mini` run on a laptop would read the laptop's
    // process table and kickstart the laptop's units under another host's name.
    let hostname = crate::providers::vast::system_hostname();
    let this_machine = registry
        .lookup_self(&hostname)
        .map_err(|error| format!("cannot resolve this machine in the registry: {error}"))?
        .map(|target| target.name.clone());
    if this_machine.as_deref() != Some(target_name) {
        return Err(format!(
            "registry.{REVISIT_POLICY_KEY} authorises units on {target_name} and this machine \
             ({hostname}) \
             resolves to {}; which image a process is executing is readable only on the machine \
             holding that process, so nothing was read and nothing was restarted",
            this_machine.as_deref().unwrap_or("no registry target")
        ));
    }
    // One lock over observe -> record -> kickstart -> settle -> record.
    //
    // What it prevents is OVERLAP: two reconciles running at the same time
    // would each observe the same stale unit against the same unchanged
    // identity pair and each spend a restart on it, because neither would see
    // the other's ledger write. It is not a rate limit and there is no time
    // window. Sequential invocations are separate ticks and each may act on
    // one unit — a different one, since the unit this tick handled is now
    // either on its declared file or barred by its own record.
    let Some(_lock) = crate::release_agent::acquire_state_lock(&scope.state_dir, LOCK_STEM)? else {
        return Ok(Some(RevisitReport {
            host: target_name.to_string(),
            acted: None,
            skipped: Vec::new(),
            busy: true,
        }));
    };
    let mut ledger = load_ledger(&scope.state_dir, target_name)?;
    let observations =
        service::observe_unit_image_scan(target, Some(target_name), chrono::Utc::now().timestamp());
    let plan = revisit_plan(&observations, &owned, &ledger);
    let Some(pick) = plan.pick else {
        // Nothing to restart and nothing an operator has to act on: every
        // authorised unit is on its declared file, or the only skips are
        // settled records already dated in the ledger and annotated on the
        // doctor row. Saying so once per tick, forever, is how a feature that
        // exists to make a condition legible becomes the noise its own signal
        // is lost in — so opting in must not add a permanent log line to a
        // healthy host.
        if plan.skipped.iter().all(|(_, skip)| skip.is_settled()) {
            return Ok(None);
        }
        return Ok(Some(RevisitReport {
            host: target_name.to_string(),
            acted: None,
            skipped: plan.skipped,
            busy: false,
        }));
    };
    let attempted_at = chrono::Utc::now().to_rfc3339();
    // Write the intent BEFORE the side effect, and refuse the side effect if
    // the write fails. A ledger that records an attempt only once its outcome
    // is known loses the attempt to any crash or write failure in the window
    // that follows, and the next tick then kickstarts the same unit again —
    // the hot loop, arriving precisely when the host is already unhealthy.
    // Refusing here costs one deferred repair; the alternative costs an
    // unbounded restart loop.
    record(
        &scope.state_dir,
        &mut ledger,
        &pick,
        AttemptOutcome::Attempting,
        &attempted_at,
        "about to issue launchctl kickstart -k".to_string(),
    )
    .map_err(|error| {
        format!(
            "{} was NOT restarted because the attempt could not be recorded first, and an \
             unrecorded restart is one this host would repeat every tick: {error}",
            pick.unit
        )
    })?;
    let (outcome, service_target) = match service::kickstart_local_unit(&pick.unit, &pick.unit_path)
    {
        Ok(service_target) => {
            let after = settle(target, target_name, &pick.unit, pick.pid).await;
            (
                AttemptOutcome::Observed(refresh_outcome(&pick.running, after.as_ref())),
                service_target,
            )
        }
        // Recorded as what it was. A refused restart is an attempt this host
        // has spent — re-issuing the same refused command every tick is the
        // hot loop this module is bounded against — but it observed nothing,
        // so it must not borrow an outcome word that claims a second read.
        Err(reason) => (AttemptOutcome::RestartRefused, format!("refused: {reason}")),
    };
    // Replaces the `Attempting` record in place, on the same identity pair, so
    // the ledger holds one row per unit either way.
    record(
        &scope.state_dir,
        &mut ledger,
        &pick,
        outcome,
        &attempted_at,
        service_target,
    )?;
    Ok(Some(RevisitReport {
        host: target_name.to_string(),
        acted: Some((pick, outcome)),
        skipped: plan.skipped,
        busy: false,
    }))
}

/// Everything `registry doctor` needs to annotate its `stale-unit-image` rows,
/// read ONCE per local doctor pass.
///
/// Built before the row loop rather than inside it: the ledger is one file and
/// one host-wide document, so opening it per row would be one read per finding
/// for one answer.
pub(crate) struct RevisitAnnotations {
    /// Every label the document authorises on this target, whether or not the
    /// contract resolved.
    declared: BTreeMap<String, String>,
    /// The state the agent will consult, or why it cannot be read.
    ///
    /// A `Result` and never an `Option`, because the failure is the finding: a
    /// ledger the agent cannot read is a host where the revisit pass will not
    /// act, and a doctor that dropped the reason would report the stale unit
    /// while silently omitting why nothing is coming for it. That is the exact
    /// defect — an unread state rendered as a clean one — that this whole
    /// check exists to remove.
    state: Result<RevisitLedger, String>,
}

/// The annotations for one host, or `None` when there is nothing to annotate.
///
/// `None` on every host today, because no target declares
/// `release_unit_image_revisit`, so [`declared_units`] is empty before a file is
/// opened. `local_units` is the host this process runs on: the ledger is a
/// local file, so a row about another host gets no clause.
pub(crate) fn annotations(
    document: &Value,
    target_name: &str,
    local_units: Option<&str>,
) -> Option<RevisitAnnotations> {
    if local_units != Some(target_name) {
        return None;
    }
    // A malformed policy cannot identify an authorised label reliably, so it
    // has no unit clause; `builds_refusing_registry` still reports the parser's
    // exact refusal through the existing `build-refuses-registry` finding.
    let policy = policy(document).ok().flatten()?;
    let declared = declared_units(&policy, target_name);
    if declared.is_empty() {
        return None;
    }
    // Refuse to open the ledger until the whole policy contract is valid. A
    // parseable policy still identifies its authorised labels, so an unsafe
    // path, platform mismatch or ownership contradiction is stated on those
    // rows as unresolved state rather than silently disappearing.
    let state = match validate_registry_contract(document) {
        Err(reason) => Err(reason),
        Ok(()) => match host_scope(&policy, target_name) {
            Ok(Some(scope)) => load_ledger(&scope.state_dir, target_name),
            Ok(None) => {
                Err("no product resolves a revisit state directory for this host".to_string())
            }
            Err(reason) => Err(reason),
        },
    };
    Some(RevisitAnnotations { declared, state })
}

impl RevisitAnnotations {
    /// The clause to append to one `stale-unit-image` row.
    ///
    /// Only for a unit some policy explicitly authorises on this target. A row
    /// about a unit no product named gets no clause, because the agent neither
    /// tried nor may try it, and saying otherwise would be a claim about a
    /// unit outside this feature.
    pub(crate) fn clause(&self, image: &StaleUnitImage) -> Option<String> {
        if !self.declared.contains_key(&image.unit) {
            return None;
        }
        let (running, declared) = match &image.state {
            ImageState::Unlinked { running, installed }
            | ImageState::Replaced { running, installed } => (running, installed),
            ImageState::Unread { .. } => return None,
        };
        match &self.state {
            Err(reason) => Some(format!(
                ". This unit is authorised in release_unit_image_revisit, so the release agent \
                 would \
                 normally restart it, but this host's revisit state could not be read — so \
                 whether it has already been attempted is unknown and the agent will not act \
                 until that is resolved: {reason}"
            )),
            Ok(ledger) => ledger
                .barring(&image.unit, running, declared)
                .map(RevisitAttempt::clause),
        }
    }
}
