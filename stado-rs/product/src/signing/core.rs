use super::constants::{DEVELOPER_ID, DEVELOPMENT, MACH_O_MAGICS};
use crate::common::{capture, checked};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// What [`inspect`] found a signing target to be; reports carry it as
/// `state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureState {
    /// Neither a Mach-O file nor a bundle: nothing to sign.
    NotNative,
    Missing,
    /// Signing is a macOS matter; this host is not macOS.
    NotApplicable,
    Unsigned,
    Invalid,
    /// Signed for this build alone, with no Apple identity.
    Adhoc,
    /// Signed, but not by an Apple Development or Developer ID identity.
    Untrusted,
    Stable,
}

impl SignatureState {
    /// Whether an installation may keep a target in this state: stable code,
    /// or a target signing does not apply to.
    pub fn acceptable(self) -> bool {
        matches!(self, Self::Stable | Self::NotNative | Self::NotApplicable)
    }

    /// Whether signing applies to the target at all: native code on macOS.
    pub fn applies(self) -> bool {
        !matches!(self, Self::NotNative | Self::NotApplicable)
    }

    /// The state a report carries, refused when it is none of these.
    pub fn of(report: &Value) -> Result<Self> {
        Self::deserialize(&report["state"])
            .with_context(|| format!("signature report state {}", report["state"]))
    }

    /// Whether `report` says its target is stably signed; a report whose
    /// state is none `inspect` writes is not.
    pub fn is_stable(report: &Value) -> bool {
        Self::of(report).is_ok_and(|state| state == Self::Stable)
    }
}

/// The Apple bundle types signing descends into, by their extension.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum BundleExtension {
    App,
    Framework,
    Xpc,
    Appex,
}

pub fn command(program: &str, args: &[&str], require_success: bool) -> Result<Output> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(unix)]
    unsafe {
        let account = libc::getpwuid(libc::geteuid());
        if account.is_null() || (*account).pw_dir.is_null() {
            bail!("cannot resolve the effective account's keychain home");
        }
        command.env(
            "HOME",
            std::ffi::CStr::from_ptr((*account).pw_dir)
                .to_string_lossy()
                .as_ref(),
        );
    }
    if require_success {
        checked(&mut command)
    } else {
        capture(&mut command)
    }
}

pub fn native(path: &Path) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let mut bytes = [0; 4];
    let count = File::open(path)?.read(&mut bytes)?;
    Ok(count == bytes.len() && MACH_O_MAGICS.contains(&bytes))
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    let expanded = match path.to_str().and_then(|p| p.strip_prefix("~/")) {
        Some(suffix) => {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(suffix)
        }
        None => path.to_path_buf(),
    };
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        Ok(std::env::current_dir()?.join(expanded))
    }
}

/// The code identifier of program `name` of `product`, in the convention
/// every Wisent name follows: `com.wisent.<product>` for the program the
/// product is named after, `com.wisent.<product>.<name>` for any other.
pub fn identifier(product: &str, name: &str) -> Result<String> {
    let identifier = if name == product {
        format!("{PREFIX}{product}")
    } else {
        format!("{PREFIX}{product}.{name}")
    };
    validate_identifier(&identifier)?;
    Ok(identifier)
}

const PREFIX: &str = "com.wisent.";
const RETIRED: &str = "ai.wisent.";

/// Names under `ai.wisent.` that are shared by every product rather than a
/// product's app: the one Wisent identity every desktop app signs in with
/// (its Keychain item, its helper, its access group). They keep their
/// meaning under the convention: `com.wisent.identity`.
const SHARED: [&str; 1] = ["identity"];

/// Retired identifiers that keep their spelling. The shared identity's
/// Keychain helper is the one program the signed-in session's Keychain item
/// trusts, by its designated requirement, which names its identifier: every
/// person's item was written by `ai.wisent.identity.keychain-helper`. Signed
/// under another identifier, macOS asked each person for the Keychain
/// password before any app could read the session, and a person who denied
/// it was signed out of every app at once. So it is never renamed, by the
/// signer or by `signing convention`.
const KEPT: [&str; 1] = ["ai.wisent.identity.keychain-helper"];

/// The identifier `kept` is named by now: one in the retired `ai.wisent.`
/// spelling, which broke the `com.wisent.<product>` convention, read as
/// the same program under the convention (`ai.wisent.oko.oko` is
/// `com.wisent.oko`, `ai.wisent.lem.LemCLI` is `com.wisent.lem.LemCLI`,
/// `ai.wisent.brama.desktop` is `com.wisent.brama.desktop`). A bare
/// `ai.wisent.<product>` was only ever a product's desktop app, so it is
/// `com.wisent.<product>.desktop` (lower case, as products are named):
/// `com.wisent.<product>` is the product's own program, and an app signed
/// with the program's identifier would share its privacy decisions. A
/// shared name (`SHARED`) keeps its own. Any other identifier is
/// unchanged. A signature carrying the retired spelling moves to this one
/// once, and is never refused as an identity change for it.
pub fn current(kept: &str) -> String {
    if KEPT.contains(&kept) {
        return kept.to_string();
    }
    match kept.strip_prefix(RETIRED) {
        Some(rest) => match rest.split_once('.') {
            Some((product, name)) if product == name => format!("{PREFIX}{product}"),
            Some((product, name)) => format!("{PREFIX}{product}.{name}"),
            None if SHARED.contains(&rest) => format!("{PREFIX}{rest}"),
            None => format!("{PREFIX}{}.desktop", rest.to_ascii_lowercase()),
        },
        None => kept.to_string(),
    }
}

/// Whether a signature that carried `kept` may be replaced by one carrying
/// `next`: the same identifier, the convention name of a retired `ai.wisent.`
/// one (`current`), or a `com.wisent.` name for a nested program that had a
/// bare one (`skarbiec`, `brama-runtime`, `libllama`), which named no owner
/// at all. Any other change is an identity change and is refused.
pub fn continues(kept: &str, next: &str) -> bool {
    kept == next
        || current(kept) == next
        || (!kept.contains('.') && next.starts_with(PREFIX))
}

pub fn validate_identifier(identifier: &str) -> Result<()> {
    if identifier.is_empty()
        || !identifier.as_bytes()[0].is_ascii_alphanumeric()
        || !identifier
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        bail!("invalid code identifier: {identifier}");
    }
    Ok(())
}

/// Refuse a product's own identifier that is not a `com.wisent.` name: the
/// convention is checked where a product's program or app is signed, so a
/// product that declares another spelling is refused at its first
/// signature instead of shipping it. Bundles nested in an app keep the
/// identifiers their makers gave them and are not checked here.
pub fn conventional(identifier: &str) -> Result<()> {
    validate_identifier(identifier)?;
    if !identifier.starts_with(PREFIX) && !KEPT.contains(&identifier) {
        bail!(
            "code identifier {identifier} is not a {PREFIX}<product> name; every Wisent program \
             and app is signed {PREFIX}<product> or {PREFIX}<product>.<name>{}",
            match identifier.starts_with(RETIRED) {
                true => format!(" ({identifier} is {} now)", current(identifier)),
                false => String::new(),
            }
        );
    }
    Ok(())
}

pub fn bundle(path: &Path) -> bool {
    path.is_dir()
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                BundleExtension::deserialize(Value::String(extension.to_owned())).is_ok()
            })
}

pub fn inspect(path: &Path) -> Result<Value> {
    let path = absolute(path)?;
    let mut report = json!({"path": path, "state": SignatureState::NotNative, "identifier": null,
        "team": null, "authority": null, "requirement": null, "error": null, "hardened_runtime": null});
    if !path.exists() {
        return failed(
            report,
            SignatureState::Missing,
            &format!("signing target does not exist: {}", path.display()),
        );
    }
    if !cfg!(target_os = "macos") {
        report["state"] = json!(SignatureState::NotApplicable);
        return Ok(report);
    }
    if !native(&path)? && !bundle(&path) {
        return Ok(report);
    }
    let pathname = path.to_str().context("signing path is not UTF-8")?;
    let shown = command(
        "/usr/bin/codesign",
        &["--display", "--verbose=4", "-r-", pathname],
        false,
    )?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&shown.stdout),
        String::from_utf8_lossy(&shown.stderr)
    );
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        if let Some((field, value)) = line.split_once('=') {
            fields.entry(field).or_insert(value);
        }
        if let Some((_, value)) = line.split_once("designated => ") {
            report["requirement"] = json!(value);
        }
        if let Some(flags) = line.strip_prefix("CodeDirectory ").and_then(|value| {
            value
                .split_whitespace()
                .find(|word| word.starts_with("flags="))
        }) {
            report["hardened_runtime"] =
                json!(flags.split_once('(').is_some_and(|(_, names)| names
                    .trim_end_matches(')')
                    .split(',')
                    .any(|name| name == "runtime")));
        }
    }
    report["identifier"] = json!(fields.get("Identifier"));
    report["team"] = json!(fields.get("TeamIdentifier"));
    report["authority"] = json!(fields.get("Authority"));
    report["cdhash"] = json!(fields.get("CDHash"));
    if !shown.status.success() {
        return failed(report, SignatureState::Unsigned, text.trim());
    }
    let verified = command(
        "/usr/bin/codesign",
        &["--verify", "--strict", "--all-architectures", pathname],
        false,
    )?;
    if !verified.status.success() {
        return failed(
            report,
            SignatureState::Invalid,
            String::from_utf8_lossy(&verified.stderr).trim(),
        );
    }
    if fields.get("Signature") == Some(&"adhoc")
        || report["requirement"]
            .as_str()
            .is_some_and(|r| r.contains("cdhash "))
    {
        return failed(
            report,
            SignatureState::Adhoc,
            "code identity is tied to this build, not an Apple signing identity",
        );
    }
    let authority = report["authority"].as_str().unwrap_or("");
    if !(authority.starts_with(DEVELOPER_ID) || authority.starts_with(DEVELOPMENT))
        || report["team"].is_null()
        || report["team"] == "not set"
    {
        return failed(
            report,
            SignatureState::Untrusted,
            "code has no Apple Development or Developer ID Application identity",
        );
    }
    let anchored = command(
        "/usr/bin/codesign",
        &[
            "--verify",
            "--strict",
            "-R",
            "=anchor apple generic",
            pathname,
        ],
        false,
    )?;
    if !anchored.status.success() {
        return failed(
            report,
            SignatureState::Untrusted,
            String::from_utf8_lossy(&anchored.stderr).trim(),
        );
    }
    report["state"] = json!(SignatureState::Stable);
    Ok(report)
}

fn failed(mut report: Value, state: SignatureState, error: &str) -> Result<Value> {
    report["state"] = json!(state);
    report["error"] = json!(error);
    Ok(report)
}

pub fn compatible(path: &Path, previous: &Value) -> Result<()> {
    if SignatureState::of(previous)? != SignatureState::Stable {
        return Ok(());
    }
    let kept = previous["requirement"]
        .as_str()
        .context("stable identity has no designated requirement")?;
    // A move `continues` accepts (a retired `ai.wisent.` name to its
    // convention name, a bare nested name to a `com.wisent.` one) keeps every
    // other clause of the installed requirement — anchor, team, certificate
    // — and names the candidate's identifier: the installed requirement
    // names the old one, which no renamed candidate satisfies, so the move
    // was refused as an identity change though the same signer made it.
    let candidate = inspect(path)?;
    let requirement = match (previous["identifier"].as_str(), candidate["identifier"].as_str()) {
        (Some(old), Some(new)) if old != new && continues(old, new) => {
            // codesign quotes an identifier holding a `.`; a bare one stands
            // unquoted.
            let Some(clause) = [format!("identifier \"{old}\""), format!("identifier {old} ")]
                .into_iter()
                .find(|clause| kept.contains(clause.as_str()))
            else {
                bail!("the installed requirement of {old} does not name it ({kept}), so the move to {new} cannot keep it");
            };
            let named = match clause.ends_with(' ') {
                true => format!("identifier \"{new}\" "),
                false => format!("identifier \"{new}\""),
            };
            kept.replacen(&clause, &named, 1)
        }
        _ => kept.to_string(),
    };
    command(
        "/usr/bin/codesign",
        &[
            "--verify",
            "--strict",
            "-R",
            &format!("={requirement}"),
            path.to_str().context("non-UTF8 signing target")?,
        ],
        true,
    )?;
    Ok(())
}
