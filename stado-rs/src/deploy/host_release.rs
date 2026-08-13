//! `stado host release TARGET --binary NAME --version X.Y.Z` — put one
//! declared, managed binary onto one registry host.
//!
//! NO Python original, and no Rust original either: `ARCHITECTURE.md` says
//! outright that "no system in the pack currently owns 'get this build onto
//! that host'. That is a gap". [`crate::deploy::host_inventory`] reads what a
//! host HAS, [`crate::targets::ComputeTarget`] declares what it SHOULD have,
//! and until this module nothing closed the two.
//!
//! It is not a new idea. Weles already ships the pattern this follows step
//! for step (`weles/scripts/worker/deploy/README.md`, "macOS worker +
//! auto-deploy"): fetch the canonical platform manifest and adjacent archive
//! through `/api/release/object`, verify the archive SHA-256, check the
//! required layout, stage the selected member under a versioned directory,
//! and only after every artifact is verified repoint the active release and
//! restart the unit.
//! A missing or mismatched release archive leaves the currently active
//! release untouched and aborts the deployment. That
//! sentence is the whole design; everything below is it, applied to one
//! managed binary instead of a worker tree.
//!
//! What that buys, stated as contracts rather than intentions:
//!
//! - **`--binary` is a closed compile-time table** ([`MANAGED_BINARIES`]).
//!   The operator's word SELECTS an entry; it never becomes part of a path,
//!   a URI segment or a script word. This is [`crate::deploy::host_exec`]'s
//!   rule, kept: "the operator's words select a fixed argv entry and never
//!   join the command line".
//! - **`--version` is an exact coordinate**, not a channel and not an
//!   alias. `latest` is a legal path segment, which is exactly why nothing
//!   here resolves one — see [`crate::release::canonical_coordinate`].
//! - **The digest comes from the canonical release manifest.** The control
//!   plane reads `release-manifest-<platform>.json` through the same Stado release API
//!   and storage contract that serves the artifact, validates its immutable
//!   product/version/platform/source-commit identity, and carries that digest
//!   to the host. Missing or malformed catalog data is a refusal.
//! - **Delivery executes a declaration; it does not replace one.** A host
//!   that declares no version for the binary, or declares a different one
//!   than `--version` names, is refused. Deciding what a host should run is
//!   the registry's job.
//! - **Verification strictly precedes activation, and that ordering is
//!   structural rather than careful.** The remote work is three separate
//!   programs on the shared channel — probe, stage, activate — and the
//!   activate program is only ever issued after the stage program reported
//!   a verified artifact. A failed fetch, a mismatched digest, a staged file
//!   that does not report the requested version: each leaves the running
//!   version untouched, because nothing has touched `$HOME/.stado/bin` yet.
//!   Splitting the phases is what makes the ordering observable at the
//!   [`Runner`] seam instead of buried inside one long shell script.
//! - **Activation is one `rename(2)`.** The staged artifact is hard-linked
//!   into a pending name beside the live one and renamed over it, so the
//!   active binary is the exact staged inode and there is no window in which
//!   `$HOME/.stado/bin/<name>` is half-written. It stays a REGULAR file on
//!   purpose: `host inventory` refuses to read through a symlink, so
//!   publishing a symlink here would blind the command that reports what is
//!   installed.
//!
//! What this command does NOT do, each for a reason:
//!
//! - it does not build, clone, fetch a tag, run a package manager or consult
//!   a channel pointer — Weles's auto-deploy does none of those either, and
//!   a host-side build is a host-side toolchain to keep alive;
//! - it does not choose a version, pick "the newest", or write the registry.
//!   Declaration is upstream of delivery on purpose: an automaton that
//!   deploys without knowing the intended state is a faster way to break
//!   production;
//! - it does not deliver more than one binary per invocation. Weles's
//!   auto-deploy stages a worker and two browsers together because they are
//!   one runtime; two independently versioned CLIs are not;
//! - it does not roll back. There is nothing to roll back from: a failure
//!   happens before activation, so the previous version is still the active
//!   one. A rollback is `host release` naming the previous version, which is
//!   why the versioned staging tree is kept rather than pruned;
//! - it does not restart a unit it invented. The unit is looked up in the
//!   registry's own declared service set ([`service::declared_services`])
//!   and restarted through the shipped `service restart` program. A binary
//!   with no declared unit — `skarbiec`, a CLI rather than a daemon — is
//!   activated and reported as having no unit, not silently "restarted".

use serde_json::{json, Map, Value};

use super::{host_channel, local_install, service};
use super::{shlex_quote, CommandOutput, DeployError, Runner};
use crate::targets::ComputeTarget;

/// `status` when the requested version was staged, verified and activated.
pub const RELEASED_STATUS: &str = "released";
/// `status` when the host already runs the requested version. Nothing was
/// fetched, staged, activated or restarted.
pub const ALREADY_ACTIVE_STATUS: &str = "already_active";
/// `status` for a `--dry-run`: the plan was built and the host was probed
/// read-only. No mutating program was sent.
pub const PLANNED_STATUS: &str = "planned";

/// The remote marker prefix, in the tab-delimited `STADO_*` protocol
/// [`crate::deploy::host_recovery::parse_output`] established.
pub const MARKER: &str = "STADO_RELEASE";

/// The registry key declaring, per binary name, the exact version a host is
/// supposed to be running. Named here only so a refusal can tell an
/// operator where to write the declaration.
pub const MANAGED_VERSIONS_KEY: &str = "managed_versions";

/// One binary this command is allowed to deliver.
///
/// A closed table, not a lookup: an operator naming something not in here
/// gets a refusal and the list, the same shape `host exec` answers an
/// unapproved command with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedBinary {
    /// The name `--binary` matches exactly, and the file name under
    /// `$HOME/.stado/bin`.
    pub name: &'static str,
    /// The `stado://releases/<product>/...` segment the artifact lives under.
    pub product: &'static str,
    /// The argument that makes it print its version.
    pub version_argument: &'static str,
    /// `plain` (one line, version as the last word) or `json` (an object
    /// with a `version` member). Both shapes are real:
    /// `host inventory` had to learn the same distinction after reporting
    /// `{` as skarbiec's version.
    pub version_shape: &'static str,
    /// The [`local_install`] kind whose label names the unit that runs this
    /// binary on a host, or `None` for a binary that is not a daemon. The
    /// label is built with [`local_install::label`] rather than spelled out,
    /// so it cannot drift from the one the installer actually creates.
    pub unit_kind: Option<&'static str>,
    /// Why this entry is safe to deliver: what it is and what runs it.
    pub why: &'static str,
}

/// Every binary `stado host release` may put on a host.
///
/// Exactly the two `host inventory` already reports under
/// `$HOME/.stado/bin`, and for the same reason: those are the two this fleet
/// installs, versions and reads back. Adding a third is a code change with a
/// justification in `why`, not a flag.
pub const MANAGED_BINARIES: &[ManagedBinary] = &[
    ManagedBinary {
        name: "stado",
        product: "stado",
        version_argument: "--version",
        version_shape: "plain",
        unit_kind: Some("agent"),
        why: "the fleet's own control binary; the per-host agent LaunchAgent runs it",
    },
    ManagedBinary {
        name: "skarbiec",
        product: "skarbiec",
        version_argument: "version",
        version_shape: "json",
        unit_kind: None,
        why: "the credential CLI; invoked per call by launchers, so no unit owns it",
    },
];

/// The platforms a managed binary is published for.
///
/// Closed for the same reason [`MANAGED_BINARIES`] is: the platform is a
/// coordinate segment, and an operator-supplied segment is an
/// operator-supplied path. These are the two
/// [`crate::deploy::bootstrap::REMOTE_INSTALL_SCRIPT`] maps the remote
/// kernel and architecture onto, so the two commands cannot disagree about
/// what a platform is called.
pub const PLATFORMS: &[&str] = &["darwin-arm64", "linux-amd64"];

/// The default platform, matching `host install-release`'s own default.
pub const DEFAULT_PLATFORM: &str = "darwin-arm64";

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// The allowlist, as an operator reads it after a refusal.
pub fn allowed_binaries() -> String {
    MANAGED_BINARIES
        .iter()
        .map(|entry| format!("  {} — {}", entry.name, entry.why))
        .collect::<Vec<String>>()
        .join("\n")
}

/// Resolve `--binary` against [`MANAGED_BINARIES`].
pub fn managed_binary(name: &str) -> Result<&'static ManagedBinary, DeployError> {
    MANAGED_BINARIES
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            DeployError(format!(
                "{name:?} is not a stado-managed binary. Deliverable binaries:\n{}",
                allowed_binaries()
            ))
        })
}

/// Resolve `--platform` against [`PLATFORMS`].
pub fn managed_platform(platform: &str) -> Result<&'static str, DeployError> {
    PLATFORMS
        .iter()
        .find(|candidate| **candidate == platform)
        .copied()
        .ok_or_else(|| {
            DeployError(format!(
                "{platform:?} is not a published release platform; expected one of {}",
                PLATFORMS.join(", ")
            ))
        })
}

/// True for an exact semantic version: three numeric identifiers without
/// leading zeros, plus an optional prerelease.
///
/// Build metadata (`+...`) is rejected rather than tolerated, because a `+`
/// is not a legal release coordinate segment
/// ([`crate::release::canonical_coordinate`]) — a version this accepts and
/// the store cannot address would be a refusal deferred to the host.
pub fn is_exact_semver(version: &str) -> bool {
    let (core, prerelease) = match version.split_once('-') {
        Some((core, rest)) => (core, Some(rest)),
        None => (version, None),
    };
    let mut parts = core.split('.');
    let numeric = |token: &str| {
        !token.is_empty()
            && token.bytes().all(|byte| byte.is_ascii_digit())
            && (token == "0" || !token.starts_with('0'))
    };
    let (Some(major), Some(minor), Some(patch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if !numeric(major) || !numeric(minor) || !numeric(patch) {
        return false;
    }
    if let Some(prerelease) = prerelease {
        if prerelease.is_empty() {
            return false;
        }
        for identifier in prerelease.split('.') {
            let alphanumeric = identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
            if identifier.is_empty() || !alphanumeric {
                return false;
            }
            if identifier.bytes().all(|byte| byte.is_ascii_digit()) && !numeric(identifier) {
                return false;
            }
        }
    }
    crate::release::canonical_coordinate(version)
}

/// True for a lowercase hex SHA-256.
pub fn is_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The version the registry declares this host must run for this binary.
///
/// One accessor, [`ComputeTarget::declared_version`], shared with the
/// reconciliation `host inventory` reports. Delivery must never carry its
/// own reading of the declaration: two readings that can disagree turn "the
/// host is behind" and "the delivery is refused" into independent answers
/// to the same question.
fn declared_version<'a>(target: &'a ComputeTarget, binary: &str) -> Option<&'a str> {
    target
        .declared_version(binary)
        .filter(|version| !version.is_empty())
}

// ---------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------

/// What an operator asked for, before any of it has been checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseRequest {
    pub binary: String,
    pub version: String,
    pub platform: String,
    /// Commit and digest stated by the canonical immutable release manifest.
    pub source_commit: String,
    pub sha256: String,
    /// The public Stado origin serving immutable releases.
    pub release_api: String,
    pub dry_run: bool,
}

/// A checked request: every refusal below has already been made, so the
/// remote programs can be built from it without re-deciding anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePlan {
    pub managed: &'static ManagedBinary,
    pub version: String,
    pub platform: String,
    pub sha256: String,
    pub source_commit: String,
    pub release_api: String,
    pub declared_version: String,
    pub dry_run: bool,
}

impl ReleasePlan {
    /// The exact object the host will fetch.
    /// The exact immutable archive the host will fetch.
    pub fn release_uri(&self) -> String {
        format!(
            "stado://releases/{}/{}/{}/{}-v{}-{}.tar.gz",
            self.managed.product,
            self.version,
            self.platform,
            self.managed.product,
            self.version,
            self.platform
        )
    }

    pub fn archive_name(&self) -> String {
        format!(
            "{}-v{}-{}.tar.gz",
            self.managed.product, self.version, self.platform
        )
    }

    /// Where the verified artifact is kept, unchanged, after delivery.
    pub fn staged_path(&self) -> String {
        format!(
            "$HOME/.stado/releases/{}/{}/{}/{}",
            self.managed.name, self.version, self.platform, self.managed.name
        )
    }

    /// The active path an operator (and `host inventory`) reads.
    pub fn active_path(&self) -> String {
        format!("$HOME/.stado/bin/{}", self.managed.name)
    }
}

/// Every refusal this command makes before it touches a host.
///
/// All of them are made here, on the control plane, and none of them depend
/// on anything the host says. A request that cannot be delivered correctly
/// should cost zero ssh connections and change nothing.
pub fn plan(target: &ComputeTarget, request: &ReleaseRequest) -> Result<ReleasePlan, DeployError> {
    let managed = managed_binary(&request.binary)?;
    if !is_exact_semver(&request.version) {
        return Err(DeployError(format!(
            "{:?} is not an exact version; --version takes a semantic version such as 0.5.1, \
             never a channel, an alias or a range. A release coordinate is immutable",
            request.version
        )));
    }
    let platform = managed_platform(&request.platform)?;
    if target.release_platform != platform {
        return Err(DeployError(format!(
            "target {:?} declares release_platform {}, not {platform}",
            target.name, target.release_platform
        )));
    }
    if !is_sha256(&request.sha256) {
        return Err(DeployError(
            "the canonical release manifest carries an invalid SHA-256".to_string(),
        ));
    }
    if !matches!(request.source_commit.len(), 40 | 64)
        || !request
            .source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DeployError(
            "the canonical release manifest carries an invalid source_commit".to_string(),
        ));
    }
    if !request.release_api.starts_with("https://")
        || request
            .release_api
            .bytes()
            .any(|byte| byte.is_ascii_whitespace())
    {
        return Err(DeployError(
            "canonical STADO_API_URL must be a whitespace-free HTTPS URL".to_string(),
        ));
    }
    let Some(declared) = declared_version(target, managed.name) else {
        return Err(DeployError(format!(
            "the registry declares no {} version for target {:?}; declare it under \
             {MANAGED_VERSIONS_KEY} first. Delivery carries out a declaration, it does not \
             stand in for one",
            managed.name, target.name
        )));
    };
    if declared != request.version {
        return Err(DeployError(format!(
            "the registry declares {} {declared} for target {:?}, not {}. Change the \
             declaration if that is the intent; delivering against it would make the \
             registry describe a host it no longer describes",
            managed.name, target.name, request.version
        )));
    }
    Ok(ReleasePlan {
        managed,
        version: request.version.clone(),
        platform: platform.to_string(),
        sha256: request.sha256.clone(),
        source_commit: request.source_commit.clone(),
        release_api: request.release_api.trim_end_matches('/').to_string(),
        declared_version: declared.to_string(),
        dry_run: request.dry_run,
    })
}

// ---------------------------------------------------------------------------
// The remote programs
// ---------------------------------------------------------------------------

/// The field sanitizer and the marker printer, shared by all three programs.
///
/// Lifted from [`crate::deploy::host_inventory::REMOTE_INVENTORY_SCRIPT`]
/// with its finding intact: this used to be a command substitution around
/// `tr`/`cut`, four forks per field, and on a host out of process slots the
/// forks failed, the substitution produced the empty string, and a report of
/// blanks went out claiming every field had been read. Builtins only, the
/// answer returned through a variable, and the sanitizer proves itself
/// against a fixed probe before anything is reported.
pub const SANITIZE_PRELUDE: &str = r##"set -eu
LC_ALL=C
export LC_ALL
field_limit=200
newline='
'

sanitize() {
  sanitize_rest="$1"
  sanitized=""
  sanitize_count=0
  while [ -n "$sanitize_rest" ] && [ "$sanitize_count" -lt "$field_limit" ]; do
    sanitize_tail=${sanitize_rest#?}
    sanitize_char=${sanitize_rest%"$sanitize_tail"}
    sanitize_rest=$sanitize_tail
    case "$sanitize_char" in
      [A-Za-z0-9]|' '|.|,|:|';'|/|@|_|+|=|%|'('|')'|-)
        sanitized="$sanitized$sanitize_char"
        ;;
      *)
        sanitized="$sanitized?"
        ;;
    esac
    sanitize_count=$((sanitize_count + 1))
  done
  if [ -z "$sanitized" ] && [ -n "$1" ]; then
    sanitized='?'
    sanitizer_state=broken
  fi
}

sanitizer_state=ok
sanitize 'probe-Value_1.2'
if [ "$sanitized" != 'probe-Value_1.2' ]; then
  sanitizer_state=broken
fi
sanitize 'a"b'
if [ "$sanitized" != 'a?b' ]; then
  sanitizer_state=broken
fi
sanitize_long=0123456789
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize "$sanitize_long"
if [ "${#sanitized}" -ne "$field_limit" ]; then
  sanitizer_state=broken
fi

# Every value leaving this host goes through here, so there is one place to
# forget and it is not a call site. The key is always a literal of the
# program below; only the value can come off the host.
say() {
  sanitize "$2"
  printf '%s\t%s\t%s\n' STADO_RELEASE "$1" "$sanitized"
}

# The bare version a managed binary declares. `stado --version` answers one
# plain line ("stado 0.5.1"); `skarbiec version` answers a JSON object. Both
# are read by shape and reduced to the bare coordinate, because the bare
# coordinate is what the registry declares and what this command compares.
read_version() {
  read_version_path="$1"
  read_version_value=""
  read_version_state=missing
  # -L first, never -f first: -f follows the link, so a symlink to another
  # binary would be executed as if it were the managed one.
  if [ -L "$read_version_path" ]; then
    read_version_state=refused_symlink
    return 0
  fi
  if [ ! -f "$read_version_path" ]; then
    return 0
  fi
  if [ ! -x "$read_version_path" ]; then
    read_version_state=not_executable
    return 0
  fi
  if read_version_output=$("$read_version_path" "$version_argument" 2>/dev/null); then
    :
  else
    read_version_state=version_failed
    return 0
  fi
  if [ -z "$read_version_output" ]; then
    read_version_state=version_empty
    return 0
  fi
  if [ "$version_shape" = json ]; then
    case "$read_version_output" in
      *'"version"'*)
        read_version_rest=${read_version_output#*'"version"'}
        read_version_gap=${read_version_rest%%'"'*}
        case "$read_version_rest" in
          *'"'*)
            case "$read_version_gap" in
              *[!:[:space:]]*) ;;
              *)
                read_version_rest=${read_version_rest#*'"'}
                read_version_value=${read_version_rest%%'"'*}
                ;;
            esac
            ;;
        esac
        ;;
    esac
  else
    read_version_line=${read_version_output%%"$newline"*}
    read_version_value=${read_version_line##* }
  fi
  if [ -z "$read_version_value" ]; then
    read_version_state=version_unparsable
  else
    read_version_state=reported
  fi
}
"##;

/// Phase one: what is on the host right now.
///
/// This program creates, moves, removes and overwrites nothing. That is what
/// makes `--dry-run` genuinely dry: the dry run is this program and nothing
/// else, so "planned but not applied" is a property of which programs were
/// sent, not a flag a longer script promises to honour.
pub const REMOTE_PROBE_BODY: &str = r##"
stado_release_step=probe
stado_home="$HOME/.stado"
active_path="$stado_home/bin/$binary"
staged_path="$stado_home/releases/$binary/$version/$platform/$binary"

# The host's own platform, from the kernel, in the spelling
# bootstrap's remote install script uses. A plan built for one platform must
# never be applied on another, and the host is the only authority on which
# one it is.
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) host_platform=darwin-arm64 ;;
  Linux-x86_64) host_platform=linux-amd64 ;;
  *) host_platform=unsupported ;;
esac
say platform "$host_platform"

read_version "$active_path"
say active_state "$read_version_state"
say active_version "$read_version_value"

if [ -L "$staged_path" ]; then
  staged_state=refused_symlink
elif [ -f "$staged_path" ]; then
  staged_state=present
elif [ -e "$staged_path" ]; then
  staged_state=not_regular
else
  staged_state=absent
fi
say staged_state "$staged_state"
say sanitizer "$sanitizer_state"
say step probe
"##;

/// Phase two: fetch the release archive, verify its catalog digest, extract
/// exactly the fixed managed-binary member, verify its declared version, and stage it.
pub const REMOTE_STAGE_BODY: &str = r##"
stado_release_step=stage
stado_home="$HOME/.stado"
staged_dir="$stado_home/releases/$binary/$version/$platform"
staged_path="$staged_dir/$binary"
archive_path="$staged_dir/.$archive_name.incoming"
incoming="$staged_dir/.$binary.incoming"

case "$release_api" in
  https://*) ;;
  *) say fetch refused_not_https; exit 1 ;;
esac
for required in /usr/bin/curl /usr/bin/openssl /usr/bin/tar; do
  if [ ! -x "$required" ]; then
    say fetch "missing_${required##*/}"
    exit 1
  fi
done

/bin/mkdir -p "$staged_dir"
/bin/rm -f "$archive_path" "$incoming"
if /usr/bin/curl -fsSL --get \
  --data-urlencode "uri=stado://releases/$product/$version/$platform/$archive_name" \
  "$release_api/api/release/object" -o "$archive_path"; then
  say fetch ok
else
  /bin/rm -f "$archive_path" "$incoming"
  say fetch failed
  exit 1
fi

digest_line=$(/usr/bin/openssl dgst -sha256 -r "$archive_path")
actual_sha256=${digest_line%% *}
say sha256 "$actual_sha256"
if [ "$actual_sha256" != "$expected_sha256" ]; then
  /bin/rm -f "$archive_path" "$incoming"
  say verify mismatch
  exit 1
fi
say verify ok

member_count=0
while IFS= read -r member; do
  if [ "$member" = "$binary" ]; then
    member_count=$((member_count + 1))
  fi
done <<EOF
$(/usr/bin/tar -tzf "$archive_path")
EOF
if [ "$member_count" -ne 1 ]; then
  /bin/rm -f "$archive_path" "$incoming"
  say layout archive_member_missing_or_duplicated
  exit 1
fi
if ! /usr/bin/tar -xOzf "$archive_path" "$binary" > "$incoming"; then
  /bin/rm -f "$archive_path" "$incoming"
  say layout archive_extract_failed
  exit 1
fi
/bin/rm -f "$archive_path"
if [ ! -s "$incoming" ]; then
  /bin/rm -f "$incoming"
  say layout empty
  exit 1
fi
/bin/chmod 755 "$incoming"
read_version "$incoming"
if [ "$read_version_state" != reported ]; then
  /bin/rm -f "$incoming"
  say layout "$read_version_state"
  exit 1
fi
if [ "$read_version_value" != "$version" ]; then
  /bin/rm -f "$incoming"
  say layout version_mismatch
  exit 1
fi
say layout ok

/bin/mv -f "$incoming" "$staged_path"
say staged "$version"
say step stage
"##;

/// Phase three: atomically activate the version-checked file staged from the
/// digest-verified archive. Re-reading the version makes this phase refuse a
/// replaced or corrupt staged file independently of the caller's ordering.
pub const REMOTE_ACTIVATE_BODY: &str = r##"
stado_release_step=activate
stado_home="$HOME/.stado"
bin_dir="$stado_home/bin"
active_path="$bin_dir/$binary"
staged_path="$stado_home/releases/$binary/$version/$platform/$binary"
pending="$bin_dir/.$binary.pending"

if [ -L "$staged_path" ] || [ ! -f "$staged_path" ] || [ ! -x "$staged_path" ]; then
  say verify staged_missing
  exit 1
fi
read_version "$staged_path"
if [ "$read_version_state" != reported ] || [ "$read_version_value" != "$version" ]; then
  say verify staged_version_mismatch
  exit 1
fi
say verify ok

/bin/mkdir -p "$bin_dir"
/bin/rm -f "$pending"
/bin/ln "$staged_path" "$pending"
/bin/chmod 755 "$pending"
/bin/mv -f "$pending" "$active_path"
say activated "$version"
say step activate
"##;

/// The checked coordinates one remote program is bound to.
fn bindings(plan: &ReleasePlan) -> String {
    format!(
        "binary={}\nproduct={}\nversion={}\nplatform={}\narchive_name={}\nexpected_sha256={}\n\
         release_api={}\nversion_argument={}\nversion_shape={}\n",
        shlex_quote(plan.managed.name),
        shlex_quote(plan.managed.product),
        shlex_quote(&plan.version),
        shlex_quote(&plan.platform),
        shlex_quote(&plan.archive_name()),
        shlex_quote(&plan.sha256),
        shlex_quote(&plan.release_api),
        shlex_quote(plan.managed.version_argument),
        shlex_quote(plan.managed.version_shape),
    )
}

/// The read-only probe program for one plan.
pub fn probe_script(plan: &ReleasePlan) -> String {
    format!("{}{SANITIZE_PRELUDE}{REMOTE_PROBE_BODY}", bindings(plan))
}

/// The fetch-verify-stage program for one plan.
pub fn stage_script(plan: &ReleasePlan) -> String {
    format!("{}{SANITIZE_PRELUDE}{REMOTE_STAGE_BODY}", bindings(plan))
}

/// The activation program for one plan.
pub fn activate_script(plan: &ReleasePlan) -> String {
    format!("{}{SANITIZE_PRELUDE}{REMOTE_ACTIVATE_BODY}", bindings(plan))
}

// ---------------------------------------------------------------------------
// Reading the host back
// ---------------------------------------------------------------------------

/// The `STADO_RELEASE` markers of one program's stdout, in order.
///
/// Matched with a slice pattern the way
/// [`crate::deploy::service::parse_markers`] matches its own, so a marker
/// with the wrong arity is ignored rather than mis-read, and a chatty login
/// shell contributes nothing.
pub fn markers(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|line| match host_channel::marker_fields(line).as_slice() {
            [MARKER, key, value] => Some(((*key).to_string(), (*value).to_string())),
            _ => None,
        })
        .collect()
}

/// One marker's value, or the empty string.
pub fn marker<'a>(markers: &'a [(String, String)], key: &str) -> &'a str {
    markers
        .iter()
        .find(|(name, _)| name == key)
        .map_or("", |(_, value)| value.as_str())
}

/// What went wrong in a program that did not finish.
///
/// The last marker it managed to emit, which is always the one that decided
/// to exit, falling back to the transport's own last word when the program
/// never spoke at all.
fn step_failure(markers: &[(String, String)], output: &CommandOutput) -> String {
    match markers.last() {
        Some((key, value)) => format!("{key} {value}"),
        None => host_channel::last_error_line(output, "ssh failed"),
    }
}

/// One executed phase, as the report carries it.
fn step_entry(step: &str, state: &str, detail: Option<String>) -> Value {
    let mut entry = Map::new();
    entry.insert("step".to_string(), json!(step));
    entry.insert("state".to_string(), json!(state));
    if let Some(detail) = detail {
        entry.insert("detail".to_string(), json!(detail));
    }
    Value::Object(entry)
}

/// Close a report as a failure at one phase, leaving everything the earlier
/// phases established in place.
fn fail(report: &mut Map<String, Value>, exit_code: i32, error: String) -> Value {
    report.insert("exit_code".to_string(), json!(exit_code));
    report.insert(
        "status".to_string(),
        json!(host_channel::FAILED_STATUS.to_string()),
    );
    report.insert("error".to_string(), json!(error));
    Value::Object(std::mem::take(report))
}

/// The registry-declared unit that runs this binary on this host, if the
/// registry declares one.
///
/// The label is built with [`local_install::label`], the same function the
/// installer names the unit with, and then has to be FOUND in
/// [`service::declared_services`]. Both halves matter: deriving the label
/// keeps it from drifting, and requiring the declaration keeps this command
/// from restarting a unit nobody said existed.
pub fn declared_unit(
    target: &ComputeTarget,
    managed: &ManagedBinary,
) -> Option<service::ManagedService> {
    let kind = managed.unit_kind?;
    let label = local_install::label(kind, &target.name);
    service::declared_services(target)
        .into_iter()
        .find(|declared| declared.matches(&label))
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// Deliver one managed binary to one already-resolved registry target.
///
/// Split out from [`release_host`] so the whole command — every refusal,
/// every phase, and the order the phases run in — is exercisable through the
/// [`Runner`] seam with no registry and no host.
pub async fn release_target(
    target: &ComputeTarget,
    request: &ReleaseRequest,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let plan = plan(target, request)?;

    let mut report = host_channel::base_report(target);
    report.insert("source_commit".to_string(), json!(plan.source_commit));
    report.insert("binary".to_string(), json!(plan.managed.name));
    report.insert("version".to_string(), json!(plan.version));
    report.insert("platform".to_string(), json!(plan.platform));
    report.insert("declared_version".to_string(), json!(plan.declared_version));
    report.insert("release_uri".to_string(), json!(plan.release_uri()));
    report.insert("sha256".to_string(), json!(plan.sha256));
    report.insert("staged_path".to_string(), json!(plan.staged_path()));
    report.insert("active_path".to_string(), json!(plan.active_path()));
    report.insert("dry_run".to_string(), json!(plan.dry_run));
    let unit = declared_unit(target, plan.managed);
    report.insert(
        "unit".to_string(),
        unit.as_ref()
            .map_or(Value::Null, |declared| json!(declared.unit_id())),
    );
    let mut steps: Vec<Value> = Vec::new();

    // Phase one: read the host. Read-only, and the only phase a dry run runs.
    let probe = host_channel::run_script(target, &probe_script(&plan), runner).await?;
    let probe_markers = markers(&probe.stdout);
    if !probe.ok() || marker(&probe_markers, "step") != "probe" {
        steps.push(step_entry(
            "probe",
            host_channel::FAILED_STATUS,
            Some(step_failure(&probe_markers, &probe)),
        ));
        report.insert("steps".to_string(), json!(steps));
        return Ok(fail(
            &mut report,
            probe.code,
            step_failure(&probe_markers, &probe),
        ));
    }
    let active_version = marker(&probe_markers, "active_version").to_string();
    let active_state = marker(&probe_markers, "active_state").to_string();
    let host_platform = marker(&probe_markers, "platform").to_string();
    report.insert("host_platform".to_string(), json!(host_platform));
    report.insert("active_version".to_string(), json!(active_version));
    report.insert("active_state".to_string(), json!(active_state));
    report.insert(
        "staged_state".to_string(),
        json!(marker(&probe_markers, "staged_state")),
    );
    steps.push(step_entry("probe", "ok", None));

    // A sanitizer that failed its own probe means every string above is
    // suspect, including the active version this command decides on.
    if marker(&probe_markers, "sanitizer") != "ok" {
        report.insert("steps".to_string(), json!(steps));
        return Ok(fail(
            &mut report,
            probe.code,
            "the host's field sanitizer failed its own probe, so the version it reported \
             cannot be trusted to decide a delivery"
                .to_string(),
        ));
    }
    // A plan built for one platform must not be applied on another, and the
    // digest is per platform, so this is a wrong-artifact check as much as a
    // wrong-machine one.
    if host_platform != plan.platform {
        report.insert("steps".to_string(), json!(steps));
        return Ok(fail(
            &mut report,
            probe.code,
            format!(
                "target runs {host_platform} and this delivery is built for {}",
                plan.platform
            ),
        ));
    }

    // Already there. Not a deployment, and not reported as one.
    if active_version == plan.version {
        report.insert("steps".to_string(), json!(steps));
        report.insert("exit_code".to_string(), json!(probe.code));
        report.insert("status".to_string(), json!(ALREADY_ACTIVE_STATUS));
        return Ok(Value::Object(report));
    }

    if plan.dry_run {
        report.insert(
            "planned_steps".to_string(),
            json!(planned_steps(&plan, unit.as_ref())),
        );
        report.insert("steps".to_string(), json!(steps));
        report.insert("exit_code".to_string(), json!(probe.code));
        report.insert("status".to_string(), json!(PLANNED_STATUS));
        return Ok(Value::Object(report));
    }

    // Phase two: fetch, verify, stage. Still nothing in $HOME/.stado/bin.
    let stage = host_channel::run_script(target, &stage_script(&plan), runner).await?;
    let stage_markers = markers(&stage.stdout);
    report.insert(
        "fetched_sha256".to_string(),
        json!(marker(&stage_markers, "sha256")),
    );
    if !stage.ok() || marker(&stage_markers, "step") != "stage" {
        let detail = step_failure(&stage_markers, &stage);
        steps.push(step_entry(
            "stage",
            host_channel::FAILED_STATUS,
            Some(detail.clone()),
        ));
        report.insert("steps".to_string(), json!(steps));
        // Said outright, because it is the question an operator asks next.
        report.insert("active_version_unchanged".to_string(), json!(true));
        return Ok(fail(&mut report, stage.code, detail));
    }
    steps.push(step_entry("stage", "ok", None));

    // Phase three: activate. Reached only because phase two verified.
    let activate = host_channel::run_script(target, &activate_script(&plan), runner).await?;
    let activate_markers = markers(&activate.stdout);
    if !activate.ok() || marker(&activate_markers, "step") != "activate" {
        let detail = step_failure(&activate_markers, &activate);
        steps.push(step_entry(
            "activate",
            host_channel::FAILED_STATUS,
            Some(detail.clone()),
        ));
        report.insert("steps".to_string(), json!(steps));
        return Ok(fail(&mut report, activate.code, detail));
    }
    steps.push(step_entry("activate", "ok", None));

    // Phase four: restart whatever the registry says runs it.
    match &unit {
        Some(declared) => {
            let restarted = service::restart_service(target, declared, runner).await?;
            if restarted.succeeded("restarted") {
                steps.push(step_entry("restart", "ok", None));
            } else {
                let detail = restarted.failure();
                steps.push(step_entry(
                    "restart",
                    host_channel::FAILED_STATUS,
                    Some(detail.clone()),
                ));
                report.insert("steps".to_string(), json!(steps));
                // The new binary IS active; only the unit still runs the old
                // image. Saying "failed" without saying that would send an
                // operator looking for an artifact that is already in place.
                report.insert("activated".to_string(), json!(true));
                return Ok(fail(&mut report, restarted.exit_code, detail));
            }
        }
        None => steps.push(step_entry(
            "restart",
            "no_declared_unit",
            Some(format!(
                "the registry declares no unit running {} on {}",
                plan.managed.name, target.name
            )),
        )),
    }

    report.insert("steps".to_string(), json!(steps));
    report.insert("exit_code".to_string(), json!(0));
    report.insert("status".to_string(), json!(RELEASED_STATUS));
    Ok(Value::Object(report))
}

/// What a `--dry-run` says it would do, in the order it would do it.
fn planned_steps(plan: &ReleasePlan, unit: Option<&service::ManagedService>) -> Vec<String> {
    let mut steps = vec![
        format!(
            "fetch {} through {}/api/release/object",
            plan.release_uri(),
            plan.release_api
        ),
        format!(
            "verify archive sha256 {} from the release manifest",
            plan.sha256
        ),
        format!(
            "extract {} and verify it declares {}",
            plan.managed.name, plan.version
        ),
        format!("stage it at {}", plan.staged_path()),
        format!(
            "re-check the staged version and atomically repoint {}",
            plan.active_path()
        ),
    ];
    steps.push(match unit {
        Some(declared) => format!("restart {}", declared.unit_id()),
        None => format!(
            "no restart: the registry declares no unit running {}",
            plan.managed.name
        ),
    });
    steps
}

pub(crate) async fn catalog_identity(
    managed: &ManagedBinary,
    version: &str,
    platform: &str,
) -> Result<(String, String), DeployError> {
    let manifest_uri = format!(
        "stado://releases/{}/{version}/{platform}/release-manifest-{platform}.json",
        managed.product
    );
    let bytes = crate::cli::storage::fetch_object(&manifest_uri)
        .await
        .map_err(|error| {
            DeployError(format!(
                "canonical release manifest is unavailable at {manifest_uri}: {error}"
            ))
        })?;
    let manifest: Value = serde_json::from_slice(&bytes)
        .map_err(|error| DeployError(format!("canonical release manifest is invalid: {error}")))?;
    let object = manifest.as_object().ok_or_else(|| {
        DeployError("canonical release manifest must be a JSON object".to_string())
    })?;
    let expected_fields = ["platform", "product", "sha256", "source_commit", "version"];
    if object.len() != expected_fields.len()
        || expected_fields
            .iter()
            .any(|field| !object.contains_key(*field))
    {
        return Err(DeployError(
            "canonical release manifest must contain exactly product, version, platform, \
             source_commit, and sha256"
                .to_string(),
        ));
    }
    let exact = |field: &str, wanted: &str| -> Result<(), DeployError> {
        let found = manifest
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if found == wanted {
            Ok(())
        } else {
            Err(DeployError(format!(
                "canonical release manifest {field} is {found:?}, expected {wanted:?}"
            )))
        }
    };
    exact("product", managed.product)?;
    exact("version", version)?;
    exact("platform", platform)?;
    let source_commit = manifest
        .get("source_commit")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let sha256 = manifest
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !is_sha256(&sha256) {
        return Err(DeployError(format!(
            "canonical release manifest has no valid SHA-256 for {}",
            managed.name
        )));
    }
    if !matches!(source_commit.len(), 40 | 64)
        || !source_commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DeployError(
            "canonical release manifest source_commit is invalid".to_string(),
        ));
    }
    Ok((source_commit, sha256))
}

/// Deliver one managed binary to one canonical registry host.
pub async fn release_host(
    target_name: &str,
    binary: &str,
    version: &str,
    dry_run: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let managed = managed_binary(binary)?;
    if !is_exact_semver(version) {
        return Err(DeployError(format!(
            "{version:?} is not an exact version; --version takes a semantic version such as \
             0.5.1, never a channel, an alias or a range. A release coordinate is immutable"
        )));
    }
    let target = host_channel::canonical_target(target_name).await?;
    let platform = managed_platform(&target.release_platform)?;
    let release_api = crate::cli::storage::release_api_origin()
        .map_err(|error| DeployError(error.to_string()))?;
    let (source_commit, sha256) = catalog_identity(managed, version, platform).await?;
    let request = ReleaseRequest {
        binary: managed.name.to_string(),
        version: version.to_string(),
        platform: platform.to_string(),
        source_commit,
        sha256,
        release_api,
        dry_run,
    };
    release_target(&target, &request, runner).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::{runner_fn, CommandOutput, CommandSpec};
    // The recording log is read and written only from inside the async
    // runner closure and the async tests, so the async lock is the right
    // one and there is no poisoning result to unwrap at any call site.
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // Both carry hex letters, so the case check below tests something.
    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER_DIGEST: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn target_with(declared: &[(&str, &str)]) -> ComputeTarget {
        ComputeTarget {
            name: "charless-mac-mini".to_string(),
            kind: "local".to_string(),
            gpu_type: None,
            slots: 1,
            ssh: Some("charles@charless-mac-mini.local".to_string()),
            region: None,
            spot: false,
            max_concurrent: None,
            team_id: None,
            notes: String::new(),
            hostnames: vec!["charless-mac-mini.local".to_string()],
            weles: None,
            disk_cleanup: None,
            env_overrides: Default::default(),
            agent_args: Vec::new(),
            vram_gb: None,
            pinned_only: false,
            managed_versions: declared
                .iter()
                .map(|(name, version)| ((*name).to_string(), (*version).to_string()))
                .collect(),
            extra: Default::default(),
        }
    }

    /// The mini as the registry describes it once it declares stado 0.5.1.
    fn target() -> ComputeTarget {
        target_with(&[("stado", "0.5.1"), ("skarbiec", "0.1.3")])
    }

    fn request() -> ReleaseRequest {
        ReleaseRequest {
            binary: "stado".to_string(),
            version: "0.5.1".to_string(),
            platform: "darwin-arm64".to_string(),
            sha256: DIGEST.to_string(),
            release_api: "https://releases.example".to_string(),
            dry_run: false,
        }
    }

    fn marker_line(key: &str, value: &str) -> String {
        format!("{MARKER}\t{key}\t{value}\n")
    }

    /// A probe answer for a host running `active`.
    fn probe_stdout(active: &str) -> String {
        format!(
            "{}{}{}{}{}",
            marker_line("platform", "darwin-arm64"),
            marker_line("active_state", "reported"),
            marker_line("active_version", active),
            marker_line("staged_state", "absent"),
            marker_line("sanitizer", "ok") + &marker_line("step", "probe"),
        )
    }

    fn stage_stdout(sha256: &str) -> String {
        format!(
            "{}{}{}{}{}",
            marker_line("fetch", "ok"),
            marker_line("sha256", sha256),
            marker_line("verify", "ok"),
            marker_line("layout", "ok"),
            marker_line("staged", "0.5.1") + &marker_line("step", "stage"),
        )
    }

    fn activate_stdout() -> String {
        format!(
            "{}{}{}{}",
            marker_line("sha256", DIGEST),
            marker_line("verify", "ok"),
            marker_line("activated", "0.5.1"),
            marker_line("step", "activate"),
        )
    }

    /// Which phase a script the runner received belongs to. Read off the
    /// `stado_release_step=` assignment each program opens its body with, so
    /// the test names the phase the same way the host does.
    fn phase(script: &str) -> String {
        for line in script.lines() {
            if let Some(step) = line.strip_prefix("stado_release_step=") {
                return step.to_string();
            }
        }
        if script.contains("STADO_SERVICE") || script.contains("launchctl") {
            return "restart".to_string();
        }
        "unknown".to_string()
    }

    /// A [`Runner`] that records the phase of every program it is handed and
    /// answers each one from `answers`. A phase with no answer fails the
    /// test rather than defaulting, so an unexpected program is loud.
    fn recording_runner(
        answers: Vec<(&'static str, CommandOutput)>,
    ) -> (Runner, Arc<Mutex<Vec<String>>>) {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let answers = Arc::new(answers);
        let runner = runner_fn(move |spec: CommandSpec| {
            let log = Arc::clone(&log);
            let answers = Arc::clone(&answers);
            async move {
                let script = spec.stdin.clone().unwrap_or_default();
                let phase = phase(&script);
                log.lock().await.push(phase.clone());
                answers
                    .iter()
                    .find(|(name, _)| *name == phase)
                    .map(|(_, output)| output.clone())
                    .ok_or_else(|| format!("no canned answer for phase {phase}"))
            }
        });
        (runner, seen)
    }

    fn ok_output(stdout: String) -> CommandOutput {
        CommandOutput {
            code: 0,
            stdout,
            stderr: String::new(),
        }
    }

    // -----------------------------------------------------------------
    // Refusals made before a host is touched
    // -----------------------------------------------------------------

    #[test]
    fn an_unmanaged_binary_is_refused_with_the_allowlist() {
        let error = managed_binary("bash").unwrap_err();
        assert!(
            error
                .0
                .starts_with("\"bash\" is not a stado-managed binary"),
            "{}",
            error.0
        );
        // The refusal has to be usable: it names what IS deliverable.
        assert!(error.0.contains("stado —"), "{}", error.0);
        assert!(error.0.contains("skarbiec —"), "{}", error.0);
        // A name that merely contains a managed one is not a managed one.
        assert!(managed_binary("stado-fix").is_err());
        assert!(managed_binary("../../bin/stado").is_err());
        assert_eq!(managed_binary("stado").unwrap().product, "stado");
    }

    #[tokio::test]
    async fn an_unmanaged_binary_never_reaches_the_host() {
        let (runner, seen) = recording_runner(vec![]);
        let mut request = request();
        request.binary = "curl".to_string();
        let error = release_target(&target(), &request, &runner)
            .await
            .unwrap_err();
        assert!(error.0.contains("is not a stado-managed binary"));
        assert!(
            seen.lock().await.is_empty(),
            "a refusal opened an ssh channel"
        );
    }

    #[test]
    fn only_an_exact_semantic_version_is_a_coordinate() {
        for good in [
            "0.5.1",
            "0.1.3",
            "0.4.392",
            "1.0.0",
            "1.2.3-rc.1",
            "10.20.30",
        ] {
            assert!(is_exact_semver(good), "{good} must be accepted");
        }
        for bad in [
            "latest", // an alias is a legal path segment; that is the danger
            "stable",
            "0.5", // not a triple
            "0.5.1.2",
            "v0.5.1",
            "0.5.x",
            "01.5.1",      // leading zero: two spellings of one version
            "0.5.1+build", // '+' is not a canonical coordinate segment
            "0.5.1-",
            "0.5.1-01",
            ">=0.5.1",
            "",
            " 0.5.1",
        ] {
            assert!(!is_exact_semver(bad), "{bad:?} must be refused");
        }
    }

    #[tokio::test]
    async fn a_version_that_is_not_a_coordinate_never_reaches_the_host() {
        let (runner, seen) = recording_runner(vec![]);
        let mut request = request();
        request.version = "latest".to_string();
        let error = release_target(&target(), &request, &runner)
            .await
            .unwrap_err();
        assert!(error.0.contains("is not an exact version"), "{}", error.0);
        assert!(
            seen.lock().await.is_empty(),
            "a refusal opened an ssh channel"
        );
    }

    #[test]
    fn a_digest_must_be_lowercase_hex_of_the_right_length() {
        assert!(is_sha256(DIGEST));
        assert!(!is_sha256(&DIGEST[..63]));
        assert!(!is_sha256(&DIGEST.to_uppercase()));
        assert!(!is_sha256(""));
        assert!(!is_sha256(&"g".repeat(64)));
    }

    #[tokio::test]
    async fn delivery_requires_a_declaration_and_obeys_it() {
        let (runner, seen) = recording_runner(vec![]);

        // Nothing declared for this host at all.
        let error = release_target(&target_with(&[]), &request(), &runner)
            .await
            .unwrap_err();
        assert!(error.0.contains("declares no stado version"), "{}", error.0);

        // Declared, but not this version. Delivery carries out a
        // declaration; it does not overrule one.
        let error = release_target(&target_with(&[("stado", "0.4.392")]), &request(), &runner)
            .await
            .unwrap_err();
        assert!(
            error.0.contains("declares stado 0.4.392") && error.0.contains("not 0.5.1"),
            "{}",
            error.0
        );

        // Declared for the other binary only.
        let error = release_target(&target_with(&[("skarbiec", "0.1.3")]), &request(), &runner)
            .await
            .unwrap_err();
        assert!(error.0.contains("declares no stado version"), "{}", error.0);

        // An empty declaration is not a declaration. A blank string in the
        // document must refuse like an absent key, not deliver "".
        let error = release_target(&target_with(&[("stado", "")]), &request(), &runner)
            .await
            .unwrap_err();
        assert!(error.0.contains("declares no stado version"), "{}", error.0);

        assert!(
            seen.lock().await.is_empty(),
            "a refusal opened an ssh channel"
        );
    }

    /// Delivery and `host inventory` must judge the same host against the
    /// same declaration, so both read it through the registry's own
    /// accessor rather than each parsing the document its own way.
    ///
    /// The one thing this wrapper adds is that a blank string is not a
    /// declaration, so that is what the fixture is built to exercise: the
    /// registry accessor answers `Some("")` for `skarbiec` and the wrapper
    /// must answer `None`. Asserting the two agree on a populated key would
    /// be asserting nothing — on that input the wrapper IS the accessor.
    #[test]
    fn the_declaration_is_read_through_the_registrys_own_accessor() {
        let document = json!({
            "name": "charless-mac-mini",
            "kind": "local",
            "ssh": "charles@charless-mac-mini.local",
            "managed_versions": {"stado": "0.5.1", "skarbiec": ""},
        });
        let target: ComputeTarget = serde_json::from_value(document).expect("registry target");

        // Declared: the wrapper hands back exactly what the registry holds,
        // so delivery and inventory cannot judge against different strings.
        assert_eq!(declared_version(&target, "stado"), Some("0.5.1"));
        assert_eq!(target.declared_version("stado"), Some("0.5.1"));

        // Present but blank: a key someone emptied instead of removing. The
        // registry reports it verbatim; delivery must treat it as no
        // declaration rather than as a declaration of "".
        assert_eq!(target.declared_version("skarbiec"), Some(""));
        assert_eq!(declared_version(&target, "skarbiec"), None);

        // Absent entirely.
        assert_eq!(declared_version(&target, "stado-fix"), None);
    }

    // -----------------------------------------------------------------
    // The phases, and the order they may run in
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn a_verified_artifact_is_staged_then_activated_then_restarted() {
        let (runner, seen) = recording_runner(vec![
            ("probe", ok_output(probe_stdout("0.4.392"))),
            ("stage", ok_output(stage_stdout(DIGEST))),
            ("activate", ok_output(activate_stdout())),
        ]);
        let report = release_target(&target(), &request(), &runner)
            .await
            .unwrap();
        assert_eq!(report["status"], RELEASED_STATUS);
        assert_eq!(report["active_version"], "0.4.392");
        assert_eq!(report["version"], "0.5.1");
        assert_eq!(
            *seen.lock().await,
            vec!["probe", "stage", "activate"],
            "the phases ran out of order"
        );
        // No unit is declared for this target in the registry document, so
        // the restart is reported as absent rather than as done.
        let steps = report["steps"].as_array().unwrap();
        assert_eq!(steps[2]["step"], "activate");
        assert_eq!(steps[3]["step"], "restart");
        assert_eq!(steps[3]["state"], "no_declared_unit");
    }

    /// The same host once the registry declares the LaunchAgent that runs
    /// the binary. The unit is not named by this command: the label is built
    /// with the installer's own [`local_install::label`] and then has to be
    /// found in the registry's declared service set.
    #[tokio::test]
    async fn a_registry_declared_unit_is_restarted_last() {
        let mut target = target();
        let label = local_install::label("agent", &target.name);
        target.extra.insert(
            service::SERVICES_KEY.to_string(),
            json!([{
                "name": "agent",
                "label": label,
                "path": "$HOME/Library/LaunchAgents/agent.plist",
                "kind": "launchd",
            }]),
        );
        let restarted = ok_output(format!("STADO_SERVICE\t{label}\trestarted\tgui/501\n"));
        let (runner, seen) = recording_runner(vec![
            ("probe", ok_output(probe_stdout("0.4.392"))),
            ("stage", ok_output(stage_stdout(DIGEST))),
            ("activate", ok_output(activate_stdout())),
            ("restart", restarted),
        ]);
        let report = release_target(&target, &request(), &runner).await.unwrap();

        assert_eq!(report["status"], RELEASED_STATUS);
        assert_eq!(report["unit"], label);
        assert_eq!(
            *seen.lock().await,
            vec!["probe", "stage", "activate", "restart"],
            "the unit was restarted out of order"
        );
        let steps = report["steps"].as_array().unwrap();
        assert_eq!(steps[3]["step"], "restart");
        assert_eq!(steps[3]["state"], "ok");
    }

    /// A unit that refused to come back is a failure, and the report has to
    /// say the artifact IS already active — otherwise an operator goes
    /// looking for a delivery that already happened.
    #[tokio::test]
    async fn a_refused_restart_fails_loudly_and_admits_the_binary_is_active() {
        let mut target = target();
        let label = local_install::label("agent", &target.name);
        target.extra.insert(
            service::SERVICES_KEY.to_string(),
            json!([{
                "name": "agent",
                "label": label,
                "path": "$HOME/Library/LaunchAgents/agent.plist",
                "kind": "launchd",
            }]),
        );
        let refused = CommandOutput {
            code: 0,
            stdout: format!("STADO_SERVICE\t{label}\trestart_failed\t5 Input/output error\n"),
            stderr: String::new(),
        };
        let (runner, _seen) = recording_runner(vec![
            ("probe", ok_output(probe_stdout("0.4.392"))),
            ("stage", ok_output(stage_stdout(DIGEST))),
            ("activate", ok_output(activate_stdout())),
            ("restart", refused),
        ]);
        let report = release_target(&target, &request(), &runner).await.unwrap();

        assert_eq!(report["status"], host_channel::FAILED_STATUS);
        assert_eq!(report["activated"], true);
        assert_eq!(
            report["error"], "restart_failed: 5 Input/output error",
            "the host's own words about the refusal were dropped"
        );
    }

    /// The ordering property, stated as the thing that must never happen: a
    /// digest that does not match must not be followed by an activation.
    #[tokio::test]
    async fn a_mismatched_digest_stops_before_the_active_version_moves() {
        let stage = CommandOutput {
            code: 1,
            stdout: marker_line("fetch", "ok")
                + &marker_line("sha256", OTHER_DIGEST)
                + &marker_line("verify", "mismatch"),
            stderr: String::new(),
        };
        // `activate` is deliberately answerable. If the command ever issued
        // it out of order the run would SUCCEED, so this test fails on the
        // recorded order rather than on a missing canned answer.
        let (runner, seen) = recording_runner(vec![
            ("probe", ok_output(probe_stdout("0.4.392"))),
            ("stage", stage),
            ("activate", ok_output(activate_stdout())),
        ]);
        let report = release_target(&target(), &request(), &runner)
            .await
            .unwrap();

        assert_eq!(report["status"], host_channel::FAILED_STATUS);
        assert_eq!(report["error"], "verify mismatch");
        assert_eq!(report["fetched_sha256"], OTHER_DIGEST);
        assert_eq!(report["active_version_unchanged"], true);
        // The host still runs what it ran before.
        assert_eq!(report["active_version"], "0.4.392");
        assert_eq!(
            *seen.lock().await,
            vec!["probe", "stage"],
            "activation was issued after a failed verification"
        );
        let steps = report["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert!(
            !steps.iter().any(|step| step["step"] == "activate"),
            "a failed verification recorded an activation step"
        );
    }

    /// The same guarantee one phase earlier: a fetch that never produced an
    /// artifact cannot reach verification, let alone activation.
    #[tokio::test]
    async fn a_failed_fetch_stops_before_verification_and_activation() {
        let stage = CommandOutput {
            code: 1,
            stdout: marker_line("fetch", "failed"),
            stderr: String::new(),
        };
        let (runner, seen) = recording_runner(vec![
            ("probe", ok_output(probe_stdout("0.4.392"))),
            ("stage", stage),
            ("activate", ok_output(activate_stdout())),
        ]);
        let report = release_target(&target(), &request(), &runner)
            .await
            .unwrap();
        assert_eq!(report["status"], host_channel::FAILED_STATUS);
        assert_eq!(report["error"], "fetch failed");
        assert_eq!(*seen.lock().await, vec!["probe", "stage"]);
    }

    #[tokio::test]
    async fn the_requested_version_being_active_is_stated_not_redelivered() {
        let (runner, seen) = recording_runner(vec![
            ("probe", ok_output(probe_stdout("0.5.1"))),
            // Answerable on purpose: an idempotent run that still fetched
            // would pass a test that only checked the status word.
            ("stage", ok_output(stage_stdout(DIGEST))),
            ("activate", ok_output(activate_stdout())),
        ]);
        let report = release_target(&target(), &request(), &runner)
            .await
            .unwrap();
        assert_eq!(report["status"], ALREADY_ACTIVE_STATUS);
        assert_eq!(report["active_version"], "0.5.1");
        assert_eq!(
            *seen.lock().await,
            vec!["probe"],
            "an already-active host was delivered to anyway"
        );
    }

    #[tokio::test]
    async fn a_dry_run_emits_no_mutating_program() {
        let mut request = request();
        request.dry_run = true;
        let (runner, seen) = recording_runner(vec![
            ("probe", ok_output(probe_stdout("0.4.392"))),
            ("stage", ok_output(stage_stdout(DIGEST))),
            ("activate", ok_output(activate_stdout())),
        ]);
        let report = release_target(&target(), &request, &runner).await.unwrap();
        assert_eq!(report["status"], PLANNED_STATUS);
        assert_eq!(
            *seen.lock().await,
            vec!["probe"],
            "a dry run sent a program that is not the read-only probe"
        );
        let planned = report["planned_steps"].as_array().unwrap();
        assert!(planned[0]
            .as_str()
            .unwrap()
            .starts_with("fetch stado://releases/stado/0.5.1/"));
        assert!(planned[1].as_str().unwrap().contains(DIGEST));
        assert!(planned[4].as_str().unwrap().contains("atomically repoint"));
    }

    /// The read-only claim, checked against the shipped program text rather
    /// than asserted in a doc comment. Scoped to the body: the shared
    /// prelude legitimately carries `2>/dev/null` around the version read,
    /// and only the body decides what a phase touches.
    #[test]
    fn the_probe_program_contains_no_mutating_word() {
        for mutating in [
            "mkdir", "curl", "/bin/mv", "/bin/rm", "/bin/ln", "/bin/cp", "chmod", ">",
        ] {
            assert!(
                !REMOTE_PROBE_BODY.contains(mutating),
                "the read-only probe program contains {mutating:?}"
            );
        }
        // And the two phases that do mutate are the two that say so, so the
        // check above cannot pass by the words having moved somewhere else.
        assert!(REMOTE_STAGE_BODY.contains("/usr/bin/curl"));
        assert!(REMOTE_ACTIVATE_BODY.contains("/bin/mv -f \"$pending\" \"$active_path\""));
    }

    #[tokio::test]
    async fn a_host_on_another_platform_is_refused_before_anything_is_fetched() {
        let probe = ok_output(
            marker_line("platform", "linux-amd64")
                + &marker_line("active_state", "reported")
                + &marker_line("active_version", "0.4.392")
                + &marker_line("staged_state", "absent")
                + &marker_line("sanitizer", "ok")
                + &marker_line("step", "probe"),
        );
        let (runner, seen) = recording_runner(vec![
            ("probe", probe),
            ("stage", ok_output(stage_stdout(DIGEST))),
        ]);
        let report = release_target(&target(), &request(), &runner)
            .await
            .unwrap();
        assert_eq!(report["status"], host_channel::FAILED_STATUS);
        assert!(report["error"]
            .as_str()
            .unwrap()
            .contains("runs linux-amd64"));
        assert_eq!(*seen.lock().await, vec!["probe"]);
    }

    /// A host whose sanitizer failed its own probe reported strings that
    /// cannot be trusted — including the active version this command would
    /// otherwise compare against. Deciding a delivery on it is the failure
    /// `host inventory` already learned to name.
    #[tokio::test]
    async fn a_broken_host_sanitizer_stops_the_delivery() {
        let probe = ok_output(
            marker_line("platform", "darwin-arm64")
                + &marker_line("active_state", "reported")
                + &marker_line("active_version", "?")
                + &marker_line("staged_state", "absent")
                + &marker_line("sanitizer", "broken")
                + &marker_line("step", "probe"),
        );
        let (runner, seen) = recording_runner(vec![
            ("probe", probe),
            ("stage", ok_output(stage_stdout(DIGEST))),
        ]);
        let report = release_target(&target(), &request(), &runner)
            .await
            .unwrap();
        assert_eq!(report["status"], host_channel::FAILED_STATUS);
        assert!(report["error"]
            .as_str()
            .unwrap()
            .contains("field sanitizer"));
        assert_eq!(*seen.lock().await, vec!["probe"]);
    }

    /// A probe that exits clean without finishing its own program is not a
    /// probe. Treating a truncated answer as "nothing is installed" would
    /// turn a broken channel into a delivery.
    #[tokio::test]
    async fn a_truncated_probe_is_a_failure_not_an_empty_host() {
        // Everything the probe reports is present and healthy, and the ONLY
        // thing missing is the marker the program prints on its last line.
        // A weaker fixture — one that also dropped the sanitizer marker —
        // would be caught by the sanitizer gate instead, and would then pass
        // whether or not the completion marker was ever checked.
        let truncated = ok_output(
            marker_line("platform", "darwin-arm64")
                + &marker_line("active_state", "reported")
                + &marker_line("active_version", "0.4.392")
                + &marker_line("staged_state", "absent")
                + &marker_line("sanitizer", "ok"),
        );
        let (runner, seen) = recording_runner(vec![
            ("probe", truncated),
            // Answerable, so a command that carried on would SUCCEED and be
            // caught by the recorded order rather than by a missing answer.
            ("stage", ok_output(stage_stdout(DIGEST))),
            ("activate", ok_output(activate_stdout())),
        ]);
        let report = release_target(&target(), &request(), &runner)
            .await
            .unwrap();
        assert_eq!(report["status"], host_channel::FAILED_STATUS);
        assert_eq!(
            *seen.lock().await,
            vec!["probe"],
            "a probe that never finished was treated as a usable answer"
        );
    }

    /// A [`Runner`] that runs whatever program the channel handed it, with
    /// `HOME` pointed at a scratch tree.
    ///
    /// This is `host_channel::run_script`'s local branch — `/bin/bash -s`
    /// with the program on stdin — so the shipped program itself is
    /// exercised rather than a paraphrase of it, and still with no remote
    /// host. Same shape `host_inventory`'s tests use for the same reason.
    fn scratch_home_runner(home: std::path::PathBuf) -> Runner {
        runner_fn(move |spec: CommandSpec| {
            let home = home.clone();
            async move {
                use tokio::io::AsyncWriteExt;

                let script = spec
                    .stdin
                    .clone()
                    .expect("the channel sends a program on stdin");
                let mut child = tokio::process::Command::new("/bin/bash")
                    .arg("-s")
                    .env("HOME", &home)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|error| error.to_string())?;
                let mut pipe = child.stdin.take().expect("stdin is piped");
                pipe.write_all(script.as_bytes())
                    .await
                    .map_err(|error| error.to_string())?;
                drop(pipe);
                let output = child
                    .wait_with_output()
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(CommandOutput {
                    code: output.status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                })
            }
        })
    }

    /// Every path under a tree with its size and modification time, sorted.
    /// The dry run's whole claim is that this is the same before and after.
    fn tree_snapshot(root: &std::path::Path) -> Vec<String> {
        fn walk(dir: &std::path::Path, into: &mut Vec<String>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .expect("readable scratch tree")
                .map(|entry| entry.expect("entry").path())
                .collect();
            entries.sort();
            for path in entries {
                let meta = std::fs::symlink_metadata(&path).expect("metadata");
                into.push(format!(
                    "{} {} {:?}",
                    path.display(),
                    meta.len(),
                    meta.modified().ok()
                ));
                if meta.is_dir() {
                    walk(&path, into);
                }
            }
        }
        let mut paths = Vec::new();
        walk(root, &mut paths);
        paths
    }

    /// The dry run, end to end, against the shipped program running under a
    /// real shell — and the proof that it changed nothing.
    ///
    /// The substring check on [`REMOTE_PROBE_BODY`] says the program has no
    /// mutating word in it; this says the program, run, leaves the host's
    /// files exactly as it found them. Only the second one is evidence.
    #[tokio::test]
    async fn a_dry_run_against_a_real_shell_leaves_the_host_untouched() {
        use std::os::unix::fs::PermissionsExt;

        // Only run where the plan's platform is this machine's; the program
        // refuses a platform mismatch, which is a different assertion.
        if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return;
        }

        let home = tempfile::tempdir().expect("tempdir");
        let bin = home.path().join(".stado").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let installed = bin.join("stado");
        std::fs::write(&installed, "#!/bin/sh\necho 'stado 0.4.392'\n").unwrap();
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();
        let before = tree_snapshot(home.path());

        let mut request = request();
        request.dry_run = true;
        let report = release_target(
            &target(),
            &request,
            &scratch_home_runner(home.path().to_path_buf()),
        )
        .await
        .unwrap();

        // The program ran, on a real shell, and read the host correctly.
        assert_eq!(report["status"], PLANNED_STATUS, "{report}");
        assert_eq!(report["host_platform"], "darwin-arm64");
        assert_eq!(report["active_version"], "0.4.392");
        assert_eq!(report["active_state"], "reported");
        assert_eq!(report["staged_state"], "absent");

        // And the host is exactly as it was: no staging directory, no
        // download, no replaced binary, not even a touched mtime.
        assert_eq!(
            tree_snapshot(home.path()),
            before,
            "the dry run wrote to the host"
        );
        assert!(
            !home.path().join(".stado").join("releases").exists(),
            "the dry run created a staging tree"
        );
    }

    // -----------------------------------------------------------------
    // The programs themselves
    // -----------------------------------------------------------------

    #[test]
    fn every_program_carries_the_sanitizer_and_its_self_test() {
        let plan = ReleasePlan {
            managed: managed_binary("skarbiec").unwrap(),
            version: "0.1.3".to_string(),
            platform: "darwin-arm64".to_string(),
            sha256: DIGEST.to_string(),
            release_api: "https://releases.example".to_string(),
            declared_version: "0.1.3".to_string(),
            dry_run: false,
        };
        for script in [
            probe_script(&plan),
            stage_script(&plan),
            activate_script(&plan),
        ] {
            assert!(script.contains("sanitize 'probe-Value_1.2'"));
            assert!(script.contains("sanitizer_state=broken"));
            // The coordinates arrive as quoted assignments; no operator word
            // is ever spliced into a program body.
            assert!(script.contains("version='0.1.3'") || script.contains("version=0.1.3"));
        }
    }

    /// The sanitizer is shell, so it is tested as shell: the shipped text,
    /// run by bash, against the values that broke the inventory command.
    #[tokio::test]
    async fn the_shipped_sanitizer_reduces_what_it_promises_to_reduce() {
        let probe = format!(
            "{SANITIZE_PRELUDE}\nsay clean 'stado 0.5.1'\nsay hostile 'a\"b$(x)`y`'\n\
             say state \"$sanitizer_state\"\n"
        );
        let output = tokio::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(&probe)
            .output()
            .await
            .expect("bash");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let seen = markers(&stdout);
        assert_eq!(marker(&seen, "state"), "ok");
        assert_eq!(marker(&seen, "clean"), "stado 0.5.1");
        // Quotes, backslashes and substitution characters cannot leave the
        // host, so nothing a corrupt file holds can shape this report.
        assert_eq!(marker(&seen, "hostile"), "a?b?(x)?y?");
    }

    /// The version reader is the other half of the shell contract: both
    /// answer shapes, read from real files by the shipped code.
    #[tokio::test]
    async fn the_shipped_version_reader_understands_both_answer_shapes() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().expect("tempdir");
        let plain = home.path().join("plain");
        std::fs::write(&plain, "#!/bin/sh\necho 'stado 0.5.1'\n").unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o700)).unwrap();
        let object = home.path().join("object");
        std::fs::write(&object, "#!/bin/sh\necho '{\"version\":\"0.1.3\"}'\n").unwrap();
        std::fs::set_permissions(&object, std::fs::Permissions::from_mode(0o700)).unwrap();

        let script = format!(
            "version_argument=--version\nversion_shape=plain\n{SANITIZE_PRELUDE}\n\
             read_version \"{plain}\"\nsay plain \"$read_version_value\"\n\
             version_shape=json\nread_version \"{object}\"\nsay object \"$read_version_value\"\n\
             version_shape=plain\nread_version \"{missing}\"\n\
             say missing \"$read_version_state\"\n",
            plain = plain.display(),
            object = object.display(),
            missing = home.path().join("absent").display(),
        );
        let output = tokio::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(&script)
            .output()
            .await
            .expect("bash");
        let seen = markers(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(marker(&seen, "plain"), "0.5.1");
        assert_eq!(marker(&seen, "object"), "0.1.3");
        // An absent binary is a state, never a blank version to interpret.
        assert_eq!(marker(&seen, "missing"), "missing");
    }

    #[test]
    fn a_plan_addresses_exactly_one_immutable_coordinate() {
        let plan = plan(&target(), &request()).unwrap();
        assert_eq!(
            plan.release_uri(),
            "stado://releases/stado/0.5.1/darwin-arm64/stado"
        );
        assert_eq!(
            plan.staged_path(),
            "$HOME/.stado/releases/stado/0.5.1/darwin-arm64/stado"
        );
        assert_eq!(plan.active_path(), "$HOME/.stado/bin/stado");
    }

    #[test]
    fn an_unpublished_platform_is_refused() {
        assert!(managed_platform("darwin-arm64").is_ok());
        assert!(managed_platform("darwin-amd64").is_err());
        assert!(managed_platform("../linux-amd64").is_err());
        let mut request = request();
        request.platform = "win32".to_string();
        assert!(plan(&target(), &request).is_err());
    }

    #[test]
    fn a_non_https_release_origin_is_refused() {
        let mut request = request();
        request.release_api = "http://releases.example".to_string();
        let error = plan(&target(), &request).unwrap_err();
        assert!(error.0.contains("HTTPS"), "{}", error.0);
    }
}
