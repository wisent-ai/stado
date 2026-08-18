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
//! sentence is the whole design; everything below is it, applied to whatever
//! the fleet declares — one program, or one worker tree.
//!
//! What that buys, stated as contracts rather than intentions:
//!
//! - **`--binary` selects a declared product** ([`products`]). The
//!   declaration is one shipped document, `stado` and `weles-worker` are two
//!   entries in it, and the operator's word SELECTS an entry; it never
//!   becomes part of a path, a URI segment or a script word. This is
//!   [`crate::deploy::host_exec`]'s rule, kept: "the operator's words select
//!   a fixed argv entry and never join the command line". Every check below
//!   is as strict for the third entry as for the first: the same manifest
//!   identity, the same digest, the same platform agreement, the same
//!   proven-to-run version readback before anything is activated.
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
//!   a verified artifact. A failed fetch, a mismatched digest, a staged
//!   artefact that does not report the requested version: each leaves the
//!   running version untouched, because nothing has touched the install root
//!   yet. Splitting the phases is what makes the ordering observable at the
//!   [`Runner`] seam instead of buried inside one long shell script.
//! - **Activation is renames, never writes in place.** A program is
//!   hard-linked into a pending name beside the live one and renamed over it,
//!   so the active binary is the exact staged inode and there is no window in
//!   which `$HOME/.stado/bin/<name>` is half-written. It stays a REGULAR file
//!   on purpose: `host inventory` refuses to read through a symlink, so
//!   publishing a symlink here would blind the command that reports what is
//!   installed. A tree is replaced path by path out of the verified staging
//!   tree, one rename each, retiring the path it replaces.
//! - **A tree delivery replaces code and nothing else.** The install root of
//!   `weles-worker` is the artefact directory itself, which also holds
//!   `recordings/`, `var/` and `.work/` — host-local state no release
//!   produced. Those paths are declared
//!   ([`crate::deploy::products::Install::Tree`]), they are never named by an
//!   activation, and an artefact that carries one of them is refused at
//!   staging rather than allowed to overwrite it.
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
//! - it does not restart a unit it invented. A product either declares a unit
//!   label alone, which has to be FOUND in the registry's own declared
//!   service set ([`service::declared_services`]) before it is touched, or
//!   declares the label together with the unit file that runs it, which is
//!   itself the statement that the unit exists. Either way the restart goes
//!   through the shipped `service restart` program, and a product with no
//!   declared unit — `skarbiec`, a CLI rather than a daemon — is activated
//!   and reported as having no unit, not silently "restarted".

use serde_json::{json, Map, Value};

use super::products::{self, Install, Product, Readback};
use super::{host_channel, service};
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

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

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
    /// The declared product this delivery carries out.
    pub product: &'static Product,
    pub version: String,
    pub platform: String,
    pub sha256: String,
    pub source_commit: String,
    pub release_api: String,
    pub declared_version: String,
    pub dry_run: bool,
}

impl ReleasePlan {
    /// The exact immutable archive the host will fetch.
    pub fn release_uri(&self) -> String {
        format!(
            "stado://releases/{}/{}/{}/{}-v{}-{}.tar.gz",
            self.product.source.product,
            self.version,
            self.platform,
            self.product.source.product,
            self.version,
            self.platform
        )
    }

    pub fn archive_name(&self) -> String {
        format!(
            "{}-v{}-{}.tar.gz",
            self.product.source.product, self.version, self.platform
        )
    }

    /// The versioned staging directory this coordinate owns. Kept after a
    /// delivery rather than pruned: naming the previous version is the only
    /// rollback this command has.
    pub fn staged_dir(&self) -> String {
        format!(
            "$HOME/.stado/releases/{}/{}/{}",
            self.product.name, self.version, self.platform
        )
    }

    /// Where the verified artefact is kept, unchanged, after delivery: the
    /// staged program itself, or the staged tree it was extracted into.
    pub fn staged_path(&self) -> String {
        match &self.product.install {
            Install::Program { .. } => format!("{}/{}", self.staged_dir(), self.product.name),
            Install::Tree { .. } => format!("{}/{TREE_DIR}", self.staged_dir()),
        }
    }

    /// The path an operator (and `host inventory`) reads the installed
    /// version out of: the active program, or the install root of a tree.
    pub fn active_path(&self) -> String {
        match &self.product.install {
            Install::Program { root } => format!("{root}/{}", self.product.name),
            Install::Tree { root, .. } => root.clone(),
        }
    }

    /// The host-local paths this delivery must leave exactly as it found
    /// them, as full paths on the host.
    pub fn preserved_paths(&self) -> Vec<String> {
        self.product.preserved_paths()
    }
}

/// Every refusal this command makes before it touches a host.
///
/// All of them are made here, on the control plane, and none of them depend
/// on anything the host says. A request that cannot be delivered correctly
/// should cost zero ssh connections and change nothing.
pub fn plan(target: &ComputeTarget, request: &ReleaseRequest) -> Result<ReleasePlan, DeployError> {
    let product = products::product(&request.binary)?;
    if !is_exact_semver(&request.version) {
        return Err(DeployError(format!(
            "{:?} is not an exact version; --version takes a semantic version such as 0.5.1, \
             never a channel, an alias or a range. A release coordinate is immutable",
            request.version
        )));
    }
    let platform = products::managed_platform(&request.platform)?;
    product.platform(platform)?;
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
    let Some(declared) = declared_version(target, &product.name) else {
        return Err(DeployError(format!(
            "the registry declares no {} version for target {:?}; declare it under \
             {MANAGED_VERSIONS_KEY} first. Delivery carries out a declaration, it does not \
             stand in for one",
            product.name, target.name
        )));
    };
    if declared != request.version {
        return Err(DeployError(format!(
            "the registry declares {} {declared} for target {:?}, not {}. Change the \
             declaration if that is the intent; delivering against it would make the \
             registry describe a host it no longer describes",
            product.name, target.name, request.version
        )));
    }
    Ok(ReleasePlan {
        product,
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

/// Phase one, program shape: what is on the host right now.
///
/// This program creates, moves, removes and overwrites nothing. That is what
/// makes `--dry-run` genuinely dry: the dry run is this program and nothing
/// else, so "planned but not applied" is a property of which programs were
/// sent, not a flag a longer script promises to honour.
pub const REMOTE_PROBE_BODY: &str = r##"
stado_release_step=probe
stado_home="$HOME/.stado"
active_path="$install_root/$binary"
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

/// Phase two, program shape: fetch the release archive, verify its catalog
/// digest, extract exactly the declared archive member, verify the version it
/// reports, and stage it.
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
while IFS= read -r archive_entry; do
  if [ "$archive_entry" = "$member" ]; then
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
if ! /usr/bin/tar -xOzf "$archive_path" "$member" > "$incoming"; then
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

/// Phase three, program shape: atomically activate the version-checked file
/// staged from the digest-verified archive. Re-reading the version makes this
/// phase refuse a replaced or corrupt staged file independently of the
/// caller's ordering.
pub const REMOTE_ACTIVATE_BODY: &str = r##"
stado_release_step=activate
stado_home="$HOME/.stado"
bin_dir="$install_root"
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

/// The directory name a staged artefact tree is kept under, inside the
/// versioned staging directory the coordinate owns.
pub const TREE_DIR: &str = "tree";

/// The version reader for a tree, added to [`SANITIZE_PRELUDE`] for a tree
/// delivery and to nothing else.
///
/// A tree has no one installed program to ask, so its version comes out of
/// one top-level member of one declared JSON file — `package.json`
/// `/version` for the Weles worker, the same field
/// `weles/.wisent-release.json` numbers the release from. The parsing rules
/// are the ones the program reader already uses on a JSON answer, including
/// the check that only whitespace and the colon sit between the key and its
/// value: `"version"` followed by `null` must read as unparsable, not as
/// whatever the next quoted member happens to be.
pub const TREE_PRELUDE: &str = r##"
read_version_file() {
  read_version_path="$1"
  read_version_value=""
  read_version_state=missing
  # -L first, never -f first: -f follows the link, so a symlink pointing at
  # another product's manifest would be read as this tree's version.
  if [ -L "$read_version_path" ]; then
    read_version_state=refused_symlink
    return 0
  fi
  if [ ! -f "$read_version_path" ]; then
    return 0
  fi
  if read_version_output=$(/bin/cat "$read_version_path" 2>/dev/null); then
    :
  else
    read_version_state=version_failed
    return 0
  fi
  if [ -z "$read_version_output" ]; then
    read_version_state=version_empty
    return 0
  fi
  read_version_key="\"$version_member\""
  case "$read_version_output" in
    *"$read_version_key"*)
      read_version_rest=${read_version_output#*"$read_version_key"}
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
  if [ -z "$read_version_value" ]; then
    read_version_state=version_unparsable
  else
    read_version_state=reported
  fi
}
"##;

/// Phase one, tree shape: what is in the install root right now.
///
/// Read-only, like its program counterpart, and it reads one thing more: the
/// top-level paths the install root holds, split into the code a delivery
/// would replace and the host-local state it must leave alone. The dry run's
/// promise is about paths on this host, so the paths come off this host
/// rather than out of an assumption on the control plane.
pub const TREE_PROBE_BODY: &str = r##"
stado_release_step=probe
stado_home="$HOME/.stado"
staged_root="$stado_home/releases/$binary/$version/$platform/tree"

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

if [ -L "$install_root" ]; then
  root_state=refused_symlink
elif [ -d "$install_root" ]; then
  root_state=present
elif [ -e "$install_root" ]; then
  root_state=not_directory
else
  root_state=absent
fi
say root_state "$root_state"

read_version_file "$install_root/$version_path"
say active_state "$read_version_state"
say active_version "$read_version_value"

if [ -L "$staged_root" ]; then
  staged_state=refused_symlink
elif [ -d "$staged_root" ]; then
  staged_state=present
elif [ -e "$staged_root" ]; then
  staged_state=not_directory
else
  staged_state=absent
fi
say staged_state "$staged_state"

if [ "$root_state" = present ]; then
  for entry_path in "$install_root"/* "$install_root"/.*; do
    [ -e "$entry_path" ] || continue
    entry=${entry_path##*/}
    case "$entry" in
      . | ..) continue ;;
    esac
    entry_kind=code
    while IFS= read -r preserved; do
      [ -n "$preserved" ] || continue
      if [ "$entry" = "$preserved" ]; then
        entry_kind=preserved
      fi
    done <<EOF
$preserve
EOF
    if [ "$entry_kind" = preserved ]; then
      say preserved_path "$entry"
    else
      say code_path "$entry"
    fi
  done
fi
say sanitizer "$sanitizer_state"
say step probe
"##;

/// Phase two, tree shape: fetch the release archive, verify its catalog
/// digest, take exactly the declared payload member out of it, unpack that
/// payload into a versioned staging tree, refuse a payload that carries a
/// host-local path, and verify the version the staged tree declares.
///
/// Nothing in this phase touches the install root, which is what makes the
/// ordering structural: a failure here leaves the running tree exactly as it
/// was, because the running tree has not been opened.
pub const TREE_STAGE_BODY: &str = r##"
stado_release_step=stage
stado_home="$HOME/.stado"
staged_dir="$stado_home/releases/$binary/$version/$platform"
staged_root="$staged_dir/tree"
archive_path="$staged_dir/.$archive_name.incoming"
payload_path="$staged_dir/.payload.incoming"
incoming="$staged_dir/.tree.incoming"

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
/bin/rm -f "$archive_path" "$payload_path"
/bin/rm -rf "$incoming"
if /usr/bin/curl -fsSL --get \
  --data-urlencode "uri=stado://releases/$product/$version/$platform/$archive_name" \
  "$release_api/api/release/object" -o "$archive_path"; then
  say fetch ok
else
  /bin/rm -f "$archive_path"
  say fetch failed
  exit 1
fi

digest_line=$(/usr/bin/openssl dgst -sha256 -r "$archive_path")
actual_sha256=${digest_line%% *}
say sha256 "$actual_sha256"
if [ "$actual_sha256" != "$expected_sha256" ]; then
  /bin/rm -f "$archive_path"
  say verify mismatch
  exit 1
fi
say verify ok

member_count=0
while IFS= read -r archive_entry; do
  if [ "$archive_entry" = "$member" ]; then
    member_count=$((member_count + 1))
  fi
done <<EOF
$(/usr/bin/tar -tzf "$archive_path")
EOF
if [ "$member_count" -ne 1 ]; then
  /bin/rm -f "$archive_path"
  say layout archive_member_missing_or_duplicated
  exit 1
fi
if ! /usr/bin/tar -xOzf "$archive_path" "$member" > "$payload_path"; then
  /bin/rm -f "$archive_path" "$payload_path"
  say layout archive_extract_failed
  exit 1
fi
/bin/rm -f "$archive_path"
if [ ! -s "$payload_path" ]; then
  /bin/rm -f "$payload_path"
  say layout empty
  exit 1
fi

/bin/mkdir -p "$incoming"
if ! /usr/bin/tar -xzf "$payload_path" -C "$incoming" --no-same-owner; then
  /bin/rm -rf "$incoming"
  /bin/rm -f "$payload_path"
  say layout payload_extract_failed
  exit 1
fi
/bin/rm -f "$payload_path"

# The payload is code, and only code. A member landing on a declared
# host-local path would be delivered over recordings or scratch state no
# release produced, so such an artefact is refused whole here rather than
# discovered halfway through an activation.
while IFS= read -r preserved; do
  [ -n "$preserved" ] || continue
  if [ -e "$incoming/$preserved" ]; then
    /bin/rm -rf "$incoming"
    say layout "artifact_carries_preserved_path_$preserved"
    exit 1
  fi
done <<EOF
$preserve
EOF

read_version_file "$incoming/$version_path"
if [ "$read_version_state" != reported ]; then
  /bin/rm -rf "$incoming"
  say layout "$read_version_state"
  exit 1
fi
if [ "$read_version_value" != "$version" ]; then
  /bin/rm -rf "$incoming"
  say layout version_mismatch
  exit 1
fi
say layout ok

/bin/rm -rf "$staged_root"
/bin/mv -f "$incoming" "$staged_root"
say staged "$version"
say step stage
"##;

/// Phase three, tree shape: replace the code in the install root out of the
/// verified staging tree, one rename per path, and leave every declared
/// host-local path exactly where it is.
///
/// The preserved paths are never named as a destination and never moved: a
/// delivery that relocated `recordings/` and put it back would be one failure
/// away from losing it. They are checked against the staged tree again here,
/// because this is the phase that can destroy state and it must refuse on its
/// own evidence rather than on the caller's ordering. The retired paths are
/// kept beside the staging tree for the same reason the staging tree is kept.
pub const TREE_ACTIVATE_BODY: &str = r##"
stado_release_step=activate
stado_home="$HOME/.stado"
staged_dir="$stado_home/releases/$binary/$version/$platform"
staged_root="$staged_dir/tree"
retired="$staged_dir/retired"

if [ -L "$staged_root" ] || [ ! -d "$staged_root" ]; then
  say verify staged_missing
  exit 1
fi
read_version_file "$staged_root/$version_path"
if [ "$read_version_state" != reported ] || [ "$read_version_value" != "$version" ]; then
  say verify staged_version_mismatch
  exit 1
fi
if [ -L "$install_root" ]; then
  say verify install_root_symlink
  exit 1
fi
if [ -e "$install_root" ] && [ ! -d "$install_root" ]; then
  say verify install_root_not_directory
  exit 1
fi
say verify ok

/bin/mkdir -p "$install_root"
/bin/rm -rf "$retired"
/bin/mkdir -p "$retired"

for staged_entry in "$staged_root"/* "$staged_root"/.*; do
  [ -e "$staged_entry" ] || continue
  entry=${staged_entry##*/}
  case "$entry" in
    . | ..) continue ;;
  esac
  while IFS= read -r preserved; do
    [ -n "$preserved" ] || continue
    if [ "$entry" = "$preserved" ]; then
      say activate "artifact_carries_preserved_path_$entry"
      exit 1
    fi
  done <<EOF
$preserve
EOF
  incoming="$install_root/.$entry.incoming"
  /bin/rm -rf "$incoming"
  /bin/cp -Rp "$staged_entry" "$incoming"
  if [ -e "$install_root/$entry" ]; then
    /bin/mv -f "$install_root/$entry" "$retired/$entry"
  fi
  /bin/mv -f "$incoming" "$install_root/$entry"
  say replaced "$entry"
done

while IFS= read -r preserved; do
  [ -n "$preserved" ] || continue
  say preserved "$preserved"
done <<EOF
$preserve
EOF

# The delivered tree, asked what it is now. A program is proven to run before
# it is activated; a tree is proven to declare the delivered version after it
# is, because the tree in the install root is the one an operator and
# `service converge` will read next.
read_version_file "$install_root/$version_path"
if [ "$read_version_state" != reported ] || [ "$read_version_value" != "$version" ]; then
  say verify installed_version_mismatch
  exit 1
fi
say activated "$version"
say step activate
"##;

/// The checked coordinates one remote program is bound to.
///
/// Every operator-facing value arrives as a quoted assignment, so no word of
/// a request is ever spliced into a program body. `install_root` is the one
/// value bound in double quotes rather than single ones, because it carries a
/// literal `$HOME` the host must expand; what makes that safe is not the
/// quoting but [`products::validate`], which admits a root of `$HOME/` plus a
/// closed alphabet of path characters and refuses everything else.
fn bindings(plan: &ReleasePlan) -> String {
    let mut bound = format!(
        "binary={}\nproduct={}\nversion={}\nplatform={}\narchive_name={}\nexpected_sha256={}\n\
         release_api={}\nmember={}\ninstall_root=\"{}\"\n",
        shlex_quote(&plan.product.name),
        shlex_quote(&plan.product.source.product),
        shlex_quote(&plan.version),
        shlex_quote(&plan.platform),
        shlex_quote(&plan.archive_name()),
        shlex_quote(&plan.sha256),
        shlex_quote(&plan.release_api),
        shlex_quote(&plan.product.source.member),
        plan.product.root(),
    );
    match &plan.product.readback {
        Readback::Program { argument, shape } => bound.push_str(&format!(
            "version_argument={}\nversion_shape={}\n",
            shlex_quote(argument),
            shlex_quote(shape.as_str()),
        )),
        Readback::JsonFile { path, .. } => bound.push_str(&format!(
            "version_path={}\nversion_member={}\npreserve={}\n",
            shlex_quote(path),
            shlex_quote(plan.product.readback.member().unwrap_or_default()),
            // One newline-delimited binding rather than a word list: a path
            // list split on IFS is a path list split on spaces too.
            shlex_quote(&plan.product.install.preserve().join("\n")),
        )),
    }
    bound
}

/// The read-only probe program for one plan.
pub fn probe_script(plan: &ReleasePlan) -> String {
    match &plan.product.install {
        Install::Program { .. } => {
            format!("{}{SANITIZE_PRELUDE}{REMOTE_PROBE_BODY}", bindings(plan))
        }
        Install::Tree { .. } => format!(
            "{}{SANITIZE_PRELUDE}{TREE_PRELUDE}{TREE_PROBE_BODY}",
            bindings(plan)
        ),
    }
}

/// The fetch-verify-stage program for one plan.
pub fn stage_script(plan: &ReleasePlan) -> String {
    match &plan.product.install {
        Install::Program { .. } => {
            format!("{}{SANITIZE_PRELUDE}{REMOTE_STAGE_BODY}", bindings(plan))
        }
        Install::Tree { .. } => format!(
            "{}{SANITIZE_PRELUDE}{TREE_PRELUDE}{TREE_STAGE_BODY}",
            bindings(plan)
        ),
    }
}

/// The activation program for one plan.
pub fn activate_script(plan: &ReleasePlan) -> String {
    match &plan.product.install {
        Install::Program { .. } => {
            format!("{}{SANITIZE_PRELUDE}{REMOTE_ACTIVATE_BODY}", bindings(plan))
        }
        Install::Tree { .. } => format!(
            "{}{SANITIZE_PRELUDE}{TREE_PRELUDE}{TREE_ACTIVATE_BODY}",
            bindings(plan)
        ),
    }
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

/// Every value one repeated marker carried, in the order the host emitted
/// them. A tree probe names one path per marker, and a path list is exactly
/// the kind of value that must not be flattened into one field: `say` caps
/// each field at 200 characters, so a joined list would be a truncated list.
pub fn marker_values<'a>(markers: &'a [(String, String)], key: &str) -> Vec<&'a str> {
    markers
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .collect()
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

/// The unit that runs this product on this host, if one is declared.
///
/// Two declarations can name it, and both are declarations rather than
/// guesses. The registry's own service set wins whenever it carries the
/// label: an operator who adopted the unit stated where its file is, and that
/// statement is newer than any shipped document. Otherwise the product
/// declaration may LOCATE the unit itself — label, kind and unit file — and
/// locating it is the statement that it exists. A product that declares only
/// a label the registry does not carry has no resolvable unit, which is
/// reported as such and never restarted: that is the rule this command has
/// always had, that it does not restart a unit nobody said existed.
pub fn declared_unit(
    target: &ComputeTarget,
    product: &Product,
) -> Option<service::ManagedService> {
    let unit = product.unit.as_ref()?;
    let label = unit.label_for(&target.name);
    if let Some(found) = service::declared_services(target)
        .into_iter()
        .find(|declared| declared.matches(&label))
    {
        return Some(found);
    }
    let path = unit.path_for(&target.name)?;
    match unit.kind.as_deref()? {
        products::UNIT_SYSTEMD => Some(service::systemd_service(
            &target.name,
            &label,
            &path,
            service::SOURCE_PRODUCT,
            "",
        )),
        _ => Some(service::launchd_service(
            &target.name,
            &label,
            &path,
            service::SOURCE_PRODUCT,
            "",
        )),
    }
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// Deliver one declared product to one already-resolved registry target.
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
    report.insert("binary".to_string(), json!(plan.product.name));
    report.insert("version".to_string(), json!(plan.version));
    report.insert("platform".to_string(), json!(plan.platform));
    report.insert("declared_version".to_string(), json!(plan.declared_version));
    report.insert("release_uri".to_string(), json!(plan.release_uri()));
    report.insert("sha256".to_string(), json!(plan.sha256));
    report.insert("staged_path".to_string(), json!(plan.staged_path()));
    report.insert("active_path".to_string(), json!(plan.active_path()));
    report.insert("install_root".to_string(), json!(plan.product.root()));
    report.insert(
        "preserved_paths".to_string(),
        json!(plan.preserved_paths()),
    );
    report.insert("dry_run".to_string(), json!(plan.dry_run));
    let unit = declared_unit(target, plan.product);
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
    // What the install root holds now, for a tree: the code a delivery
    // replaces and the host-local state it does not. Empty for a program,
    // which has neither.
    let code_paths: Vec<String> = marker_values(&probe_markers, "code_path")
        .into_iter()
        .map(str::to_string)
        .collect();
    if plan.product.install.is_tree() {
        report.insert("root_state".to_string(), json!(marker(&probe_markers, "root_state")));
        report.insert("code_paths".to_string(), json!(code_paths));
        report.insert(
            "preserved_paths_present".to_string(),
            json!(marker_values(&probe_markers, "preserved_path")),
        );
    }
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
            json!(planned_steps(&plan, unit.as_ref(), &code_paths)),
        );
        report.insert("steps".to_string(), json!(steps));
        report.insert("exit_code".to_string(), json!(probe.code));
        report.insert("status".to_string(), json!(PLANNED_STATUS));
        return Ok(Value::Object(report));
    }

    // Phase two: fetch, verify, stage. Still nothing in the install root.
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
    if plan.product.install.is_tree() {
        // The paths this delivery actually replaced, as the host named them
        // one by one. An operator asking what a tree delivery touched gets
        // the answer from the program that did it.
        report.insert(
            "replaced_paths".to_string(),
            json!(marker_values(&activate_markers, "replaced")),
        );
    }

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
                plan.product.name, target.name
            )),
        )),
    }

    report.insert("steps".to_string(), json!(steps));
    report.insert("exit_code".to_string(), json!(0));
    report.insert("status".to_string(), json!(RELEASED_STATUS));
    Ok(Value::Object(report))
}

/// What a `--dry-run` says it would do, in the order it would do it.
///
/// `code_paths` is what the read-only probe found in the install root of a
/// tree, so the paths this promises to replace and the paths it promises to
/// keep are the paths that are actually there — not a guess made on the
/// control plane about a host nobody looked at.
fn planned_steps(
    plan: &ReleasePlan,
    unit: Option<&service::ManagedService>,
    code_paths: &[String],
) -> Vec<String> {
    let readback = match &plan.product.readback {
        Readback::Program { .. } => String::new(),
        Readback::JsonFile { path, pointer } => format!(" in {path} {pointer}"),
    };
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
            "extract {} and verify it declares {}{readback}",
            plan.product.source.member, plan.version
        ),
        format!("stage it at {}", plan.staged_path()),
    ];
    match &plan.product.install {
        Install::Program { .. } => steps.push(format!(
            "re-check the staged version and atomically repoint {}",
            plan.active_path()
        )),
        Install::Tree { root, .. } => {
            steps.push(format!(
                "re-check the staged version and replace the code under {root}, one rename each, \
                 retiring what it replaces: {}",
                if code_paths.is_empty() {
                    "nothing is installed there yet".to_string()
                } else {
                    code_paths.join(", ")
                }
            ));
            steps.push(format!(
                "preserve untouched, never moved and never named as a destination: {}",
                plan.preserved_paths().join(", ")
            ));
        }
    }
    steps.push(match unit {
        Some(declared) => format!("restart {}", declared.unit_id()),
        None => format!(
            "no restart: the registry declares no unit running {}",
            plan.product.name
        ),
    });
    steps
}

/// The immutable identity the canonical release manifest states for one
/// declared product at one coordinate: its source commit and the archive
/// digest a host must reproduce.
///
/// The manifest is the only source of both. A product whose declared version
/// was never published has no manifest, and this is where that is refused —
/// on the control plane, before a host is contacted, for a dry run exactly as
/// for a delivery.
pub(crate) async fn catalog_identity(
    product: &Product,
    version: &str,
    platform: &str,
) -> Result<(String, String), DeployError> {
    let manifest_uri = format!(
        "stado://releases/{}/{version}/{platform}/release-manifest-{platform}.json",
        product.source.product
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
    exact("product", &product.source.product)?;
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
            product.name
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

/// Deliver one declared product to one canonical registry host.
pub async fn release_host(
    target_name: &str,
    binary: &str,
    version: &str,
    dry_run: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let product = products::product(binary)?;
    if !is_exact_semver(version) {
        return Err(DeployError(format!(
            "{version:?} is not an exact version; --version takes a semantic version such as \
             0.5.1, never a channel, an alias or a range. A release coordinate is immutable"
        )));
    }
    let target = host_channel::canonical_target(target_name).await?;
    let platform = products::managed_platform(&target.release_platform)?;
    product.platform(platform)?;
    let release_api = crate::cli::storage::release_api_origin()
        .map_err(|error| DeployError(error.to_string()))?;
    let (source_commit, sha256) = catalog_identity(product, version, platform).await?;
    let request = ReleaseRequest {
        binary: product.name.clone(),
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

    /// The mini, built from a registry document rather than a struct
    /// literal: the document is what the registry actually carries, and a
    /// fixture that has to be edited every time a target gains a field
    /// stops being a fixture.
    fn target_with(declared: &[(&str, &str)]) -> ComputeTarget {
        let versions: Map<String, Value> = declared
            .iter()
            .map(|(name, version)| ((*name).to_string(), json!(version)))
            .collect();
        serde_json::from_value(json!({
            "name": "charless-mac-mini",
            "kind": "local",
            "ssh": "charles@charless-mac-mini.local",
            "release_platform": "darwin-arm64",
            "hostnames": ["charless-mac-mini.local"],
            "managed_versions": versions,
        }))
        .expect("registry target")
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
            source_commit: "151ce0f907cc1e7b22a2c4e7356a4251444f4d42".to_string(),
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
    /// the binary. `stado` declares the label and not the unit file, so the
    /// label still has to be FOUND in the registry's declared service set —
    /// and the declared label is the one
    /// [`crate::deploy::local_install::label`] builds.
    #[tokio::test]
    async fn a_registry_declared_unit_is_restarted_last() {
        let mut target = target();
        let label = crate::deploy::local_install::label("agent", &target.name);
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
        let label = crate::deploy::local_install::label("agent", &target.name);
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
            product: products::product("skarbiec").expect("a declared product"),
            version: "0.1.3".to_string(),
            platform: "darwin-arm64".to_string(),
            sha256: DIGEST.to_string(),
            source_commit: "151ce0f907cc1e7b22a2c4e7356a4251444f4d42".to_string(),
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
            "stado://releases/stado/0.5.1/darwin-arm64/stado-v0.5.1-darwin-arm64.tar.gz"
        );
        assert_eq!(
            plan.staged_path(),
            "$HOME/.stado/releases/stado/0.5.1/darwin-arm64/stado"
        );
        assert_eq!(plan.active_path(), "$HOME/.stado/bin/stado");
        // A program product has no host-local state beside it, so a delivery
        // preserves nothing and says so.
        assert!(plan.preserved_paths().is_empty());
    }

    /// A platform is refused when the fleet does not publish for it at all,
    /// and — separately — when the requested product does not.
    #[test]
    fn an_unpublished_platform_is_refused() {
        let mut unpublished = request();
        unpublished.platform = "win32".to_string();
        assert!(plan(&target(), &unpublished).is_err());

        let mut linux = target_with(&[("weles-worker", "0.5.1")]);
        linux.release_platform = "linux-amd64".to_string();
        let mut request = request();
        request.binary = "weles-worker".to_string();
        request.platform = "linux-amd64".to_string();
        let error = plan(&linux, &request).unwrap_err();
        assert!(
            error.0.contains("publishes no linux-amd64 release"),
            "{}",
            error.0
        );
    }

    #[test]
    fn a_non_https_release_origin_is_refused() {
        let mut request = request();
        request.release_api = "http://releases.example".to_string();
        let error = plan(&target(), &request).unwrap_err();
        assert!(error.0.contains("HTTPS"), "{}", error.0);
    }
}
