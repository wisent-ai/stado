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
active_sha256=""
if [ ! -L "$active_path" ] && [ -f "$active_path" ]; then
  active_digest_line=$(/usr/bin/openssl dgst -sha256 -r "$active_path" 2>/dev/null || true)
  active_sha256=${active_digest_line%% *}
fi
say active_sha256 "$active_sha256"

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
staged_sha256=""
if [ "$staged_state" = present ]; then
  staged_digest_line=$(/usr/bin/openssl dgst -sha256 -r "$staged_path" 2>/dev/null || true)
  staged_sha256=${staged_digest_line%% *}
fi
say staged_sha256 "$staged_sha256"
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
reader_archive="$staged_dir/$reader_archive_name"
incoming="$staged_dir/.$binary.incoming"

# The plan enforced the scheme contract before this script existed: HTTPS
# for every target, loopback HTTP only for a host delivering from its own
# store. This guard is the host-side tripwire for the same shapes.
case "$release_api" in
  https://*|http://127.*|http://localhost|http://localhost:*|http://\[::1\]|http://\[::1\]:*) ;;
  *) say fetch refused_not_https; exit 1 ;;
esac
for required in /usr/bin/curl /usr/bin/openssl /usr/bin/tar /usr/bin/awk /usr/bin/tr /usr/bin/wc; do
  if [ ! -x "$required" ]; then
    say fetch "missing_${required##*/}"
    exit 1
  fi
done

/bin/mkdir -p "$staged_dir"
/bin/rm -f "$archive_path" "$incoming"
if ! fetch_release_object \
  "stado://releases/$product/$version/$platform/$archive_name" \
  "$archive_path"; then
  /bin/rm -f "$archive_path" "$incoming"
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
  say layout "archive_member_count_${member_count}_expected_${member}_in_${archive_name}"
  exit 1
fi
if ! /usr/bin/tar -xOzf "$archive_path" "$member" > "$incoming"; then
  /bin/rm -f "$archive_path" "$incoming"
  say layout archive_extract_failed
  exit 1
fi
if [ ! -s "$incoming" ]; then
  /bin/rm -f "$archive_path" "$incoming"
  say layout empty
  exit 1
fi
/bin/chmod 755 "$incoming"
read_version "$incoming"
if [ "$read_version_state" != reported ]; then
  /bin/rm -f "$archive_path" "$incoming"
  say layout "$read_version_state"
  exit 1
fi
if [ "$read_version_value" != "$version" ]; then
  /bin/rm -f "$archive_path" "$incoming"
  say layout version_mismatch
  exit 1
fi
say layout ok
if [ "$binary" = stado ]; then
  /bin/mv -f "$archive_path" "$reader_archive"
else
  /bin/rm -f "$archive_path"
fi


/bin/mv -f "$incoming" "$staged_path"
staged_digest_line=$(/usr/bin/openssl dgst -sha256 -r "$staged_path")
staged_sha256=${staged_digest_line%% *}
say staged_sha256 "$staged_sha256"
say staged "$version"
say step stage
"##;
/// Fetch only the immutable Stado archive needed to resume service-local
/// reader convergence. This deliberately does not extract or activate the
/// root program: a byte-attested root may already be complete while a private
/// reader remains old.
pub(super) const REMOTE_RETAIN_READER_ARCHIVE_BODY: &str = r##"
stado_release_step=retain_reader_archive
staged_dir="$HOME/.stado/releases/$binary/$version/$platform"
archive_path="$staged_dir/.$archive_name.reader-incoming"
reader_archive="$staged_dir/$reader_archive_name"
for required in /usr/bin/curl /usr/bin/openssl /usr/bin/awk /usr/bin/tr /usr/bin/wc; do
  if [ ! -x "$required" ]; then
    say fetch "missing_${required##*/}"
    exit 1
  fi
done


/bin/mkdir -p "$staged_dir"
/bin/rm -f "$archive_path"
if ! fetch_release_object \
  "stado://releases/$product/$version/$platform/$archive_name" \
  "$archive_path"; then
  /bin/rm -f "$archive_path"
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
/bin/mv -f "$archive_path" "$reader_archive"
say verify ok
say step retain_reader_archive
"##;

/// Phase three, program shape: atomically activate the version-checked file
pub(super) const REMOTE_RECHECK_STAGE_BODY: &str = r##"
stado_release_step=stage
staged_path="$HOME/.stado/releases/$binary/$version/$platform/$binary"
if [ -L "$staged_path" ] || [ ! -f "$staged_path" ] || [ ! -x "$staged_path" ]; then
  say verify staged_missing
  exit 1
fi
read_version "$staged_path"
if [ "$read_version_state" != reported ] || [ "$read_version_value" != "$version" ]; then
  say verify staged_version_mismatch
  exit 1
fi
staged_digest_line=$(/usr/bin/openssl dgst -sha256 -r "$staged_path")
staged_sha256=${staged_digest_line%% *}
say staged_sha256 "$staged_sha256"
say verify ok
say step stage
"##;

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
release_version_incoming="$bin_dir/stado.release-version.release-incoming"

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
if [ "$binary" = stado ]; then
  /bin/rm -f "$release_version_incoming"
  printf '%s\n' "$version" > "$release_version_incoming"
fi
/bin/ln "$staged_path" "$pending"
/bin/chmod 755 "$pending"
/bin/mv -f "$pending" "$active_path"
if [ "$binary" = stado ]; then
  /bin/mv -f "$release_version_incoming" "$bin_dir/stado.release-version"
fi
say activated "$version"
active_digest_line=$(/usr/bin/openssl dgst -sha256 -r "$active_path")
active_sha256=${active_digest_line%% *}
say active_sha256 "$active_sha256"

# The receipt, beside the artefact it describes.
#
# `converge` can already prove WHETHER these bytes were delivered, by
# comparing the installed file against this staged copy. It could not say
# WHO, and on 2026-08-31 that is exactly where an investigation stopped: a
# `stado` answering 0.13.19 appeared in `$HOME/.stado/bin` on the always-on
# Mac at 21:25Z, the release channel was ruled out, both operator sessions
# were ruled out, the repository's automation was ruled out, and nothing on
# the host recorded who had installed it.
#
# It lives in the version/platform directory rather than beside the active
# binary, so a receipt cannot outlive the artefact it describes or be read
# for a different one. A delivery made before this format simply has none,
# and its absence means "installed before receipts", never "suspicious":
# `staged-match` and `no-staged-copy` answer correctly without it.
receipt_dir="$stado_home/releases/$binary/$version/$platform"
if [ -d "$receipt_dir" ]; then
  printf '{"binary":"%s","version":"%s","platform":"%s","source_commit":"%s","sha256":"%s","installed_at":"%s","delivered_by":"%s"}\n' \
    "$binary" "$version" "$platform" "${source_commit:-}" "${expected_sha256:-}" \
    "$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)" "${delivered_by:-}" \
    > "$receipt_dir/release-receipt.json" 2>/dev/null || true
  /bin/chmod 600 "$receipt_dir/release-receipt.json" 2>/dev/null || true
fi
say step activate
"##;
