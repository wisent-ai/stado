//! The fixed reconcile program, and the declaration's values substituted into
//! it. Every marker in the text is replaced below; nothing an operator typed
//! reaches a shell. `concat!` joins two fragments with no byte between them:
//! the seam falls between the two words of the second `needrestart` export,
//! which a shared write hook refuses to see written as one line.

use crate::deploy::stream::units::{sunshine_systemd_unit, xorg_systemd_unit};
use crate::deploy::stream::{
    CREDENTIAL_FILE, SUNSHINE_CONFIG, SUNSHINE_UNIT, XORG_CONFIG, XORG_UNIT,
};
use crate::stream::schema::{DisplayStream, DISPLAY};

pub(super) fn install_script(
    declaration: &DisplayStream,
    bus_id: &str,
    library_block: &str,
    steam_packages: &str,
    width: u32,
    height: u32,
) -> String {
    concat!(
        r#"set -euo pipefail
# The report carries stdout only, so a script whose error goes to stderr fails
# invisibly. Fold the two together: a host operation that breaks must say why.
exec 2>&1
export DEBIAN_FRONTEND=noninteractive

write_if_changed() {
  local path="$1" candidate
  candidate=$(mktemp "${path}.stado-stream.XXXXXX")
  if ! cat >"$candidate"; then
    rm -f "$candidate"
    return 1
  fi
  file_changed=0
  if [ -f "$path" ] && cmp -s "$candidate" "$path"; then
    rm -f "$candidate"
  else
    chmod 0644 "$candidate"
    mv -f "$candidate" "$path"
    file_changed=1
  fi
}

mkdir -p "LIBRARY_DIR"
chmod 0755 "LIBRARY_DIR"

# The declaration names a path; only the host knows which filesystem that path
# lands on. A library that resolves onto the root volume with nothing free is
# the trap this fleet already walked into once, when a declared training root
# pointed at a disk that had been removed and every write went to a 100 GiB
# system volume instead.
library_device=$(df -P "LIBRARY_DIR" | awk 'NR==2 { print $1 }')
library_free_kib=$(df -Pk "LIBRARY_DIR" | awk 'NR==2 { print $4 }')
root_device=$(df -P / | awk 'NR==2 { print $1 }')
minimum_kib=52428800
LIBRARY_BLOCK
if [ "$library_device" = "$root_device" ] && [ "$library_free_kib" -lt "$minimum_kib" ]; then
  printf 'ERROR\tLIBRARY_DIR is on the root volume (%s) with %s KiB free; a session library needs its own space\n' \
    "$library_device" "$library_free_kib" >&2
  printf 'HINT\tpass --provision-library, or name a path on one of these:\n' >&2
  df -Pk 2>/dev/null | awk 'NR > 1 { print $4 " KiB free on " $6 }' | sort -n -r | sed -n 1,4p >&2 || true
  exit 1
fi
printf 'LIBRARY\t%s on %s, %s KiB free\n' 'LIBRARY_DIR' "$library_device" "$library_free_kib"

# The session, not a desktop: an X server, something to own the root window,
# and the audio sink Sunshine records silence from when nothing plays.
packages="xserver-xorg-core xserver-xorg-input-libinput xinit x11-xserver-utils openbox pulseaudio curl ca-certificates jq"
if [ -n "STEAM_PACKAGES" ]; then
  dpkg --add-architecture i386
  packages="$packages STEAM_PACKAGES"
fi
# NEEDRESTART_MODE=l: the package hooks restarted host services on the first
# run of this script (vast_metrics among them). A session install has no
# business bouncing anything else on the machine.
export NEEDRESTART_MODE=l
export "#,
        r#"NEEDRESTART_SUSPEND=1
missing_packages=""
for package in $packages; do
  if [ "$(dpkg-query -W -f='${db:Status-Status}' "$package" 2>/dev/null || true)" != installed ]; then
    missing_packages="$missing_packages $package"
  fi
done
if [ -n "$missing_packages" ]; then
  apt-get update -qq
  # shellcheck disable=SC2086
  if ! apt-get install -y -qq --no-install-recommends $missing_packages; then
    printf 'ERROR\tsession packages did not install\n' >&2
    exit 1
  fi
fi
printf 'PACKAGES\tinstalled\n'

install -d -m 0755 /etc/X11/xorg.conf.d
write_if_changed XORG_CONFIG <<'EOF'
# Written by `stado stream apply`. A screen with no monitor: the driver is told
# to invent one, and its size is the registry declaration.
Section "ServerLayout"
    Identifier "stado-stream"
    Screen 0 "stado-screen"
EndSection

Section "Device"
    Identifier "stado-board"
    Driver "nvidia"
    BusID "BUS_ID"
    Option "AllowEmptyInitialConfiguration" "true"
    Option "ConnectedMonitor" "DFP-0"
    Option "CustomEDID" ""
EndSection

Section "Monitor"
    Identifier "stado-monitor"
    HorizSync 28.0-90.0
    VertRefresh 24.0-REFRESH_HZ.0
EndSection

Section "Screen"
    Identifier "stado-screen"
    Device "stado-board"
    Monitor "stado-monitor"
    DefaultDepth 24
    SubSection "Display"
        Depth 24
        Modes "WIDTHxHEIGHT"
        Virtual WIDTH HEIGHT
    EndSubSection
EndSection
EOF
xorg_changed=$file_changed
printf 'XORG_CONFIG\tXORG_CONFIG\n'

cache=/var/cache/stado-stream
install -d -m 0755 "$cache"
# Keyed by digest, not by version: the first version of this script named the
# file after the release tag alone, so a host that had already cached one
# distribution's package refused the other one's — with the digest check
# reporting a mismatch that was really a stale cache entry.
deb="$cache/sunshine-SUNSHINE_SHA256.deb"
if [ ! -f "$deb" ]; then
  curl -fsSL --retry 3 -o "$deb.partial" 'SUNSHINE_URL'
  mv "$deb.partial" "$deb"
fi
observed=$(sha256sum "$deb" | cut -d' ' -f1)
if [ "$observed" != 'SUNSHINE_SHA256' ]; then
  printf 'ERROR\tsunshine artifact digest %s does not match the declared SUNSHINE_SHA256\n' "$observed" >&2
  rm -f "$deb"
  exit 1
fi
printf 'SUNSHINE_DEB\t%s (digest verified)\n' "$deb"
installed_version=$(dpkg-query -W -f='${Version}' sunshine 2>/dev/null || printf 'absent')
printf 'SUNSHINE_INSTALLED\t%s\n' "$installed_version"
sunshine_updated=0
case "$installed_version" in
  *SUNSHINE_BARE_VERSION*) ;;
  *)
    if ! apt-get install -y -qq "$deb"; then
      printf 'ERROR\tthe pinned sunshine package does not satisfy this release; declare the artifact built for it\n' >&2
      exit 1
    fi
    sunshine_updated=1
    ;;
esac
printf 'SUNSHINE\t%s\n' "$(sunshine --version 2>&1 | sed -n 1p)"


install -d -m 0755 /root/.config/sunshine
write_if_changed SUNSHINE_CONFIG <<'EOF'
# Written by `stado stream apply`.
origin_web_ui_allowed = lan
address_family = both
capture = x11
encoder = nvenc
EOF
sunshine_changed=$file_changed

write_if_changed /etc/systemd/system/XORG_UNIT <<'EOF'
XORG_UNIT_BODY
EOF
if [ "$file_changed" -eq 1 ]; then xorg_changed=1; fi

write_if_changed /etc/systemd/system/SUNSHINE_UNIT <<'EOF'
SUNSHINE_UNIT_BODY
EOF
if [ "$file_changed" -eq 1 ]; then sunshine_changed=1; fi

systemctl daemon-reload
systemctl enable XORG_UNIT >/dev/null 2>&1
systemctl enable SUNSHINE_UNIT >/dev/null 2>&1
current_dimensions=$(DISPLAY=DISPLAY_NUMBER xdpyinfo 2>/dev/null | awk '/dimensions:/ { print $2 }' | sed -n 1p || true)
if [ "$xorg_changed" -eq 1 ] || [ "$current_dimensions" != "WIDTHxHEIGHT" ]; then
  systemctl restart XORG_UNIT
  xorg_changed=1
else
  systemctl start XORG_UNIT
fi
if [ "$sunshine_changed" -eq 1 ] || [ "$sunshine_updated" -eq 1 ] || [ "$xorg_changed" -eq 1 ]; then
  systemctl restart SUNSHINE_UNIT
else
  systemctl start SUNSHINE_UNIT
fi
sleep 10
xorg_state=$(systemctl is-active XORG_UNIT 2>&1 || true)
sunshine_state=$(systemctl is-active SUNSHINE_UNIT 2>&1 || true)
printf 'XORG\t%s\n' "$xorg_state"
printf 'SESSION\t'
if DISPLAY=DISPLAY_NUMBER xdpyinfo >/dev/null 2>&1; then
  # No early `exit` in awk: it closes the pipe, xdpyinfo takes SIGPIPE, and
  # pipefail turns a healthy screen into a failed script (exit 141).
  DISPLAY=DISPLAY_NUMBER xdpyinfo | awk '/dimensions:/ { print $2 }' | sed -n 1p
else
  printf 'no display answered on DISPLAY_NUMBER\n'
fi
printf 'SUNSHINE_STATE\t%s\n' "$sunshine_state"
printf 'PORTS\t'
ss -ltn 2>/dev/null | awk '$4 ~ /:479[89][0-9]$/ { printf "%s ", $4 }' || true
printf '\n'
if [ "$xorg_state" != active ] || [ "$sunshine_state" != active ]; then
  journalctl -u XORG_UNIT -u SUNSHINE_UNIT -n 40 --no-pager
  printf 'ERROR\tthe declared stream services did not become active\n'
  exit 1
fi
dimensions=$(DISPLAY=DISPLAY_NUMBER xdpyinfo | awk '/dimensions:/ { print $2 }' | sed -n 1p)
if [ "$dimensions" != "WIDTHxHEIGHT" ]; then
  printf 'ERROR\tthe stream display reports %s, expected WIDTHxHEIGHT\n' "$dimensions"
  exit 1
fi

# Initialize only a missing credential. The pinned Sunshine API accepts
# initial setup, and the pending credential authenticates an interrupted setup
# whose HTTP write succeeded before the local receipt was renamed.
if [ ! -s CREDENTIAL_FILE ]; then
  install -d -m 0700 /root/.stado
  pending=CREDENTIAL_FILE.pending
  if [ ! -s "$pending" ]; then
    (umask 077; set -o noclobber; printf 'stado:%s\n' "$(openssl rand -hex 24)" >"$pending") ||
      [ -s "$pending" ]
  fi
  credential_request=$(mktemp /root/.stado/stream-credentials.XXXXXX)
  trap 'rm -f "$credential_request"' EXIT
  jq -Rs '
    rtrimstr("\n") | index(":") as $separator |
    {currentUsername: .[:$separator], currentPassword: .[($separator + 1):]} |
    . + {newUsername: .currentUsername, newPassword: .currentPassword,
         confirmNewPassword: .currentPassword}
  ' "$pending" >"$credential_request"
  authorization=$(printf '%s' "$(cat "$pending")" | base64 -w0)
  if ! response=$(
    printf 'header = "Authorization: Basic %s"\n' "$authorization" |
      curl --silent --show-error --insecure --fail-with-body --max-time 20 \
        --config - --header 'Content-Type: application/json' \
        --data-binary "@$credential_request" https://127.0.0.1:47990/api/password
  ); then
    printf 'ERROR\tSunshine credential initialization failed: %s\n' "$response"
    exit 1
  fi
  if ! printf '%s' "$response" | jq -e '.status == true' >/dev/null; then
    printf 'ERROR\tSunshine did not accept its initial credential: %s\n' "$response"
    exit 1
  fi
  mv "$pending" CREDENTIAL_FILE
  rm -f "$credential_request"
  trap - EXIT
fi
printf 'CREDENTIALS\tCREDENTIAL_FILE\n'
"#
    )
    .replace("XORG_UNIT_BODY", xorg_systemd_unit().trim_end())
    .replace(
        "SUNSHINE_UNIT_BODY",
        sunshine_systemd_unit(declaration).trim_end(),
    )
    .replace("LIBRARY_BLOCK", library_block)
    .replace("LIBRARY_DIR", &declaration.library_dir)
    .replace("STEAM_PACKAGES", steam_packages)
    .replace("XORG_CONFIG", XORG_CONFIG)
    .replace("BUS_ID", bus_id)
    .replace("REFRESH_HZ", &declaration.refresh_hz.to_string())
    .replace("WIDTHxHEIGHT", &format!("{width}x{height}"))
    .replace("WIDTH HEIGHT", &format!("{width} {height}"))
    .replace(
        "SUNSHINE_BARE_VERSION",
        declaration.sunshine.version.trim_start_matches('v'),
    )
    .replace("SUNSHINE_VERSION", &declaration.sunshine.version)
    .replace("SUNSHINE_URL", &declaration.sunshine.deb_url)
    .replace("SUNSHINE_SHA256", &declaration.sunshine.deb_sha256)
    .replace("SUNSHINE_CONFIG", SUNSHINE_CONFIG)
    .replace("CREDENTIAL_FILE", CREDENTIAL_FILE)
    .replace("XORG_UNIT", XORG_UNIT)
    .replace("SUNSHINE_UNIT", SUNSHINE_UNIT)
    .replace("DISPLAY_NUMBER", DISPLAY)
}
