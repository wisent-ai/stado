//! Remote lifecycle for one interactive display session and its stream.
//!
//! The host side of `stado stream`. Every script here is fixed text with the
//! declaration's values substituted, run through the registry SSH channel like
//! every other host operation — no operator words reach a shell.
//!
//! What it builds on a host that has boards and no monitor:
//!
//!   - an Xorg screen the driver invents (`AllowEmptyInitialConfiguration`),
//!     sized by the declaration, pinned to one board by PCI bus id;
//!   - a session on it (`openbox`, because something must own the root window
//!     and a full desktop is not the ask);
//!   - Sunshine, installed from a digest-pinned `.deb`, encoding that screen
//!     with the board's own encoder;
//!   - two systemd units, so the pair survives a reboot without a display
//!     manager and without logging anyone in.
//!
//! `pair` exists because Moonlight's PIN has to reach Sunshine's API, and the
//! only other route is a browser — which this fleet does not open on an
//! operator's machine.

use serde_json::{json, Map, Value};

use super::{host_channel, DeployError, Runner};
use crate::stream::schema::{DisplayStream, DISPLAY, SUNSHINE_HTTPS_PORT};
use crate::targets::ComputeTarget;

const XORG_UNIT: &str = "stado-stream-xorg.service";
const SUNSHINE_UNIT: &str = "stado-stream-sunshine.service";
const XORG_CONFIG: &str = "/etc/X11/xorg.conf.d/10-stado-stream.conf";
const CREDENTIAL_FILE: &str = "/root/.stado/stream-webui-credentials";

fn report(target: &ComputeTarget, output: &super::CommandOutput, ok: &str) -> Value {
    let mut body = host_channel::base_report(target);
    host_channel::finish_report(&mut body, output, ok, "stream operation failed");
    body.insert("stdout".to_string(), Value::String(output.stdout.clone()));
    Value::Object(body)
}

/// Tab-separated `KEY\tVALUE` lines from a host script, as a report object.
fn parse_fields(stdout: &str) -> Map<String, Value> {
    let mut fields = Map::new();
    for line in stdout.lines() {
        if let Some((key, value)) = line.split_once('\t') {
            let entry = fields
                .entry(key.trim().to_lowercase())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(list) = entry.as_array_mut() {
                list.push(Value::String(value.trim().to_string()));
            }
        }
    }
    // One value stays a string; repeats stay a list. A caller reading
    // `driver` should not have to know whether the host had one line or three.
    let mut flattened = Map::new();
    for (key, value) in fields {
        let collapsed = match value.as_array().map(Vec::as_slice) {
            Some([only]) => only.clone(),
            _ => value,
        };
        flattened.insert(key, collapsed);
    }
    flattened
}

/// Package installs and a `.deb` download need more than an ordinary host
/// operation's bound.
fn install_timeout() -> std::time::Duration {
    // Wide enough for apt plus a package download, narrow enough that a wedged
    // unit start is a five-minute answer rather than an hour of silence.
    host_channel::remote_timeout().saturating_mul(u8::BITS.saturating_div(2))
}

/// Can this host render and encode at all, and what would it render on?
///
/// Read-only, and it answers before anything is installed: boards and their PCI
/// bus ids, driver version, DRM nodes, encoder presence, free space on the
/// declared library volume, the tailnet address a client would dial, and whether
/// a display manager already owns the screen.
pub async fn probe(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let script = r#"set -euo pipefail
# The report carries stdout only, so a script whose error goes to stderr fails
# invisibly. Fold the two together: a host operation that breaks must say why.
exec 2>&1
printf 'HOST\t'; hostname
printf 'KERNEL\t'; uname -sr
printf 'RELEASE\t'; . /etc/os-release && printf '%s %s\n' "$ID" "$VERSION_ID"
if ! command -v nvidia-smi >/dev/null; then printf 'ERROR\tnvidia-smi missing\n'; exit 1; fi
printf 'DRIVER\t'; nvidia-smi --query-gpu=driver_version --format=csv,noheader | sed -n 1p
nvidia-smi --query-gpu=index,uuid,name,pci.bus_id,memory.total --format=csv,noheader |
  while IFS= read -r row; do printf 'BOARD\t%s\n' "$row"; done
printf 'ENCODER\t'
if nvidia-smi --query-gpu=encoder.stats.sessionCount --format=csv,noheader >/dev/null 2>&1; then
  printf 'nvenc present\n'
else
  printf 'unknown\n'
fi
nodes=""
if [ -d /dev/dri ]; then
  for node in /dev/dri/*; do nodes="$nodes$(basename "$node") "; done
fi
printf 'DRM_NODES\t%s\n' "${nodes:-none}"
printf 'DISPLAY_MANAGER\t'
if systemctl is-active --quiet gdm3 2>/dev/null || systemctl is-active --quiet lightdm 2>/dev/null || systemctl is-active --quiet sddm 2>/dev/null; then
  printf 'present (a session already owns the screen)\n'
else
  printf 'none\n'
fi
# Presence first, version second: `Xorg -version` prints its banner on stderr in
# a shape that varies, and an empty version line reads as "absent" when the
# binary is right there.
printf 'XORG_INSTALLED\t'
if command -v Xorg >/dev/null; then
  version=$(Xorg -version 2>&1 | sed -n 's/^X.Org X Server //p' | sed -n 1p || true)
  printf 'present %s\n' "${version:-(version unread)}"
else
  printf 'absent\n'
fi
printf 'SUNSHINE_INSTALLED\t'
if command -v sunshine >/dev/null; then
  printf 'present %s\n' "$(dpkg-query -W -f='${Version}' sunshine 2>/dev/null || printf 'unknown')"
else
  printf 'absent\n'
fi
printf 'APT\t'; command -v apt-get >/dev/null && printf 'present\n' || printf 'absent\n'
printf 'TAILSCALE\t'
if command -v tailscale >/dev/null; then
  tailscale ip 2>/dev/null | while IFS= read -r address; do case "$address" in *:*) ;; *) printf '%s\n' "$address"; break ;; esac; done
else
  printf 'absent\n'
fi
printf 'ROOT_FREE_KIB\t'; df -Pk / | awk 'NR==2 { print $4 }'
printf 'LIBRARY_FREE_KIB\t'; df -Pk "LIBRARY_DIR" 2>/dev/null | awk 'NR==2 { print $4 }' || printf 'unknown\n'
printf 'UNITS\t'
for unit in XORG_UNIT SUNSHINE_UNIT; do
  printf '%s=%s ' "$unit" "$(systemctl is-active "$unit" 2>&1 || true)"
done
printf '\n'
"#
    .replace("LIBRARY_DIR", &library_dir(target))
    .replace("XORG_UNIT", XORG_UNIT)
    .replace("SUNSHINE_UNIT", SUNSHINE_UNIT);
    let output = host_channel::run_script(target, &script, runner).await?;
    let mut body = report(target, &output, "probed");
    if let Some(map) = body.as_object_mut() {
        let fields = parse_fields(&output.stdout);
        map.insert("fields".to_string(), Value::Object(fields));
    }
    Ok(body)
}

fn library_dir(target: &ComputeTarget) -> String {
    target
        .display_stream
        .as_ref()
        .map(|declaration| declaration.library_dir.clone())
        .unwrap_or_else(|| crate::stream::schema::DEFAULT_LIBRARY_DIR.to_string())
}

/// Reconcile the host to its declaration: packages, screen, session, Sunshine,
/// units. Idempotent — an installed host reports what it already had.
///
/// `bus_id` is the PCI address of the declared board, resolved by the caller
/// from the probe, because Xorg addresses a card by bus id and the declaration
/// names it by driver UUID.
pub async fn install(
    target: &ComputeTarget,
    declaration: &DisplayStream,
    bus_id: &str,
    provision_library: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    declaration
        .validate(&format!("targets[{}].display_stream", target.name))
        .map_err(DeployError)?;
    let (width, height) = declaration
        .dimensions()
        .ok_or_else(|| DeployError("resolution has no dimensions".to_string()))?;
    let steam_packages = if declaration.steam {
        "steam-installer"
    } else {
        ""
    };
    // Reshaping a host's storage is not something to do silently, so the two
    // halves are separate: without the flag a library with no room is refused
    // and the largest mounts are named; with it, the declared path becomes a
    // bind mount on the largest disk-backed filesystem, the same shape this
    // host already uses for agent staging.
    let library_block = if provision_library {
        r#"if [ "$library_device" = "$root_device" ] && [ "$library_free_kib" -lt "$minimum_kib" ]; then
  # Only a real block-backed filesystem, and never a container's overlay: the
  # first version of this search picked
  # /var/lib/docker/overlay2/<id>/merged, which is one running container's
  # filesystem and vanishes with it.
  source_mount=$(awk '$3 ~ /^(ext4|ext3|xfs|btrfs|zfs|f2fs)$/ && $2 != "/" && $2 != "/boot" { print $2 }' /proc/self/mounts |
    while read -r point; do
      printf '%s %s\n' "$(df -Pk "$point" | awk 'NR==2 { print $4 }')" "$point"
    done | sort -n -r | sed -n 1p | cut -d' ' -f2)
  if [ -z "$source_mount" ]; then
    printf 'ERROR\tno disk-backed filesystem here has room for a session library\n' >&2
    exit 1
  fi
  backing="$source_mount/wisent-games"
  mkdir -p "$backing"
  if ! awk -v point='LIBRARY_DIR' '$2 == point { found = 1 } END { exit !found }' /proc/self/mounts; then
    mount --bind "$backing" "LIBRARY_DIR"
  fi
  line="$backing LIBRARY_DIR none bind 0 0 # stado-stream"
  if ! grep -Fxq "$line" /etc/fstab; then
    cp -p /etc/fstab "/etc/fstab.before-stream-library-$(date -u +%Y%m%d)"
    printf '%s\n' "$line" >>/etc/fstab
  fi
  library_device=$(df -P "LIBRARY_DIR" | awk 'NR==2 { print $1 }')
  library_free_kib=$(df -Pk "LIBRARY_DIR" | awk 'NR==2 { print $4 }')
  printf 'LIBRARY_PROVISIONED\t%s bound to LIBRARY_DIR\n' "$backing"
fi
"#
    } else {
        ""
    };
    let script = r#"set -euo pipefail
# The report carries stdout only, so a script whose error goes to stderr fails
# invisibly. Fold the two together: a host operation that breaks must say why.
exec 2>&1
export DEBIAN_FRONTEND=noninteractive

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
packages="xserver-xorg-core xserver-xorg-input-libinput xinit x11-xserver-utils openbox pulseaudio curl ca-certificates"
if [ -n "STEAM_PACKAGES" ]; then
  dpkg --add-architecture i386
  packages="$packages STEAM_PACKAGES"
fi
# NEEDRESTART_MODE=l: the package hooks restarted host services on the first
# run of this script (vast_metrics among them). A session install has no
# business bouncing anything else on the machine.
export NEEDRESTART_MODE=l
export NEEDRESTART_SUSPEND=1
apt-get update -qq
# shellcheck disable=SC2086
if ! apt-get install -y -qq --no-install-recommends $packages; then
  printf 'ERROR\tsession packages did not install\n' >&2
  exit 1
fi
printf 'PACKAGES\tinstalled\n'

install -d -m 0755 /etc/X11/xorg.conf.d
cat >XORG_CONFIG <<'EOF'
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
case "$installed_version" in
  *SUNSHINE_BARE_VERSION*) ;;
  *)
    if ! apt-get install -y -qq "$deb"; then
      printf 'ERROR\tthe pinned sunshine package does not satisfy this release; declare the artifact built for it\n' >&2
      exit 1
    fi
    ;;
esac
printf 'SUNSHINE\t%s\n' "$(sunshine --version 2>&1 | sed -n 1p)"

# Web-UI credentials: generated here, so no secret crosses the control channel
# or lands in this host's process table from outside. `stream pair` reads them
# back on this host and nowhere else.
install -d -m 0700 /root/.stado
if [ ! -s CREDENTIAL_FILE ]; then
  password=$(openssl rand -hex 24)
  printf 'stado:%s\n' "$password" >CREDENTIAL_FILE
  chmod 0600 CREDENTIAL_FILE
fi
user=$(cut -d: -f1 CREDENTIAL_FILE)
secret=$(cut -d: -f2- CREDENTIAL_FILE)
timeout 20 sunshine --creds "$user" "$secret" >/dev/null 2>&1 || printf 'CREDS\tsunshine refused the credential write; the web UI keeps its own\n'
printf 'CREDENTIALS\tCREDENTIAL_FILE\n'

install -d -m 0755 /root/.config/sunshine
cat >/root/.config/sunshine/sunshine.conf <<'EOF'
# Written by `stado stream apply`.
origin_web_ui_allowed = lan
address_family = both
capture = x11
encoder = nvenc
EOF

cat >/etc/systemd/system/XORG_UNIT <<'EOF'
[Unit]
Description=Stado stream: X server on the declared board
After=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/Xorg DISPLAY_NUMBER -config XORG_CONFIG -noreset -novtswitch -sharevts
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

cat >/etc/systemd/system/SUNSHINE_UNIT <<'EOF'
[Unit]
Description=Stado stream: Sunshine encoding the session
After=XORG_UNIT
Requires=XORG_UNIT

[Service]
Type=simple
Environment=DISPLAY=DISPLAY_NUMBER
Environment=HOME=/root
ExecStartPre=/bin/sh -c 'for _ in $(seq 30); do /usr/bin/xdpyinfo -display DISPLAY_NUMBER >/dev/null 2>&1 && exit 0; sleep 1; done; exit 1'
# openbox does not daemonise, so it belongs in the background: as an
# ExecStartPre it never returned and the unit sat in `activating` until the
# start timeout, which reads exactly like a crash that never happened.
ExecStartPre=/bin/sh -c 'setsid /usr/bin/openbox --replace --sm-disable >/tmp/stado-stream-openbox.log 2>&1 &'
ExecStart=/usr/bin/sunshine /root/.config/sunshine/sunshine.conf
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now XORG_UNIT >/dev/null 2>&1 || true
systemctl enable --now SUNSHINE_UNIT >/dev/null 2>&1 || true
sleep 8
printf 'XORG\t%s\n' "$(systemctl is-active XORG_UNIT 2>&1 || true)"
printf 'SESSION\t'
if DISPLAY=DISPLAY_NUMBER xdpyinfo >/dev/null 2>&1; then
  # No early `exit` in awk: it closes the pipe, xdpyinfo takes SIGPIPE, and
  # pipefail turns a healthy screen into a failed script (exit 141).
  DISPLAY=DISPLAY_NUMBER xdpyinfo | awk '/dimensions:/ { print $2 }' | sed -n 1p
else
  printf 'no display answered on DISPLAY_NUMBER\n'
fi
printf 'SUNSHINE_STATE\t%s\n' "$(systemctl is-active SUNSHINE_UNIT 2>&1 || true)"
printf 'PORTS\t'
ss -ltn 2>/dev/null | awk '$4 ~ /:479[89][0-9]$/ { printf "%s ", $4 }' || true
printf '\n'
"#
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
    .replace("CREDENTIAL_FILE", CREDENTIAL_FILE)
    .replace("XORG_UNIT", XORG_UNIT)
    .replace("SUNSHINE_UNIT", SUNSHINE_UNIT)
    .replace("DISPLAY_NUMBER", DISPLAY);
    let output =
        host_channel::run_script_with_timeout(target, &script, install_timeout(), runner).await?;
    let mut body = report(target, &output, "installed");
    if let Some(map) = body.as_object_mut() {
        map.insert(
            "fields".to_string(),
            Value::Object(parse_fields(&output.stdout)),
        );
    }
    Ok(body)
}

/// What the session is doing right now: units, the screen's real size, the
/// board carrying it, ports, paired clients, and room left for the library.
pub async fn status(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let script = r#"set -euo pipefail
# The report carries stdout only, so a script whose error goes to stderr fails
# invisibly. Fold the two together: a host operation that breaks must say why.
exec 2>&1
printf 'XORG\t%s\n' "$(systemctl is-active XORG_UNIT 2>&1 || true)"
printf 'SUNSHINE\t%s\n' "$(systemctl is-active SUNSHINE_UNIT 2>&1 || true)"
printf 'SESSION\t'
if DISPLAY=DISPLAY_NUMBER xdpyinfo >/dev/null 2>&1; then
  # No early `exit` in awk: it closes the pipe, xdpyinfo takes SIGPIPE, and
  # pipefail turns a healthy screen into a failed script (exit 141).
  DISPLAY=DISPLAY_NUMBER xdpyinfo | awk '/dimensions:/ { print $2 }' | sed -n 1p
else
  printf 'no display on DISPLAY_NUMBER\n'
fi
printf 'RENDERING_BOARD\t'
rendering=$(nvidia-smi --query-compute-apps=gpu_uuid,process_name --format=csv,noheader 2>/dev/null | grep -i -E 'Xorg|sunshine' | sed -n 1p || true)
printf '%s\n' "${rendering:-idle (nothing rendering yet)}"
printf 'PORTS\t'
ss -ltn 2>/dev/null | awk '$4 ~ /:479[89][0-9]$/ { printf "%s ", $4 }' || true
printf '\n'
printf 'PAIRED_CLIENTS\t'
state=/root/.config/sunshine/sunshine_state.json
if [ -r "$state" ]; then
  /usr/bin/python3 -c '
import json, sys
document = json.load(open(sys.argv[1]))
clients = document.get("root", document).get("devices") or []
print(len(clients))
' "$state" 2>/dev/null || printf 'unreadable\n'
else
  printf '0\n'
fi
printf 'LIBRARY\t'
df -Ph "LIBRARY_DIR" 2>/dev/null | awk 'NR==2 { print $1, $4 " available" }' || printf 'absent\n'
printf 'XORG_LOG\t%s\n' "$(journalctl -u XORG_UNIT --no-pager -n 3 -o cat 2>/dev/null | tr '\n' '|' || true)"
printf 'SUNSHINE_LOG\t%s\n' "$(journalctl -u SUNSHINE_UNIT --no-pager -n 3 -o cat 2>/dev/null | tr '\n' '|' || true)"
printf 'CLIENT_ENDPOINT\t'
if command -v tailscale >/dev/null; then
  tailscale ip 2>/dev/null | while IFS= read -r address; do case "$address" in *:*) ;; *) printf '%s\n' "$address"; break ;; esac; done
else
  printf 'unknown\n'
fi
"#
    .replace("XORG_UNIT", XORG_UNIT)
    .replace("SUNSHINE_UNIT", SUNSHINE_UNIT)
    .replace("LIBRARY_DIR", &library_dir(target))
    .replace("DISPLAY_NUMBER", DISPLAY);
    let output = host_channel::run_script(target, &script, runner).await?;
    let mut body = report(target, &output, "reported");
    if let Some(map) = body.as_object_mut() {
        map.insert(
            "fields".to_string(),
            Value::Object(parse_fields(&output.stdout)),
        );
        map.insert(
            "client_port".to_string(),
            Value::from(SUNSHINE_HTTPS_PORT),
        );
    }
    Ok(body)
}

/// Hand Moonlight's PIN to Sunshine.
///
/// The PIN is four digits the client just generated and it authorises exactly
/// one pairing, so it is the one operator value this surface takes. It is
/// checked for shape before it is substituted, and the web credentials never
/// leave the host.
pub async fn pair(
    target: &ComputeTarget,
    pin: &str,
    client_name: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if pin.len() != "0000".len() || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(DeployError(format!(
            "pin {pin:?} is not the four digits Moonlight shows"
        )));
    }
    if !client_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(DeployError(format!(
            "client name {client_name:?} must be letters, digits, dash or underscore"
        )));
    }
    let script = r#"set -euo pipefail
# The report carries stdout only, so a script whose error goes to stderr fails
# invisibly. Fold the two together: a host operation that breaks must say why.
exec 2>&1
[ -r CREDENTIAL_FILE ] || { printf 'ERROR\tno web credentials at CREDENTIAL_FILE; run `stado stream apply` first\n' >&2; exit 1; }
user=$(cut -d: -f1 CREDENTIAL_FILE)
secret=$(cut -d: -f2- CREDENTIAL_FILE)
status=$(curl -sS -k -o /tmp/stado-stream-pair.$$ -w '%{http_code}' \
  --max-time 20 \
  -u "$user:$secret" \
  -H 'Content-Type: application/json' \
  -X POST "https://127.0.0.1:SUNSHINE_HTTPS_PORT/api/pin" \
  --data '{"pin":"PIN","name":"CLIENT_NAME"}')
printf 'HTTP\t%s\n' "$status"
printf 'BODY\t%s\n' "$(tr -d '\n' </tmp/stado-stream-pair.$$ | cut -c1-200)"
rm -f /tmp/stado-stream-pair.$$
case "$status" in 200) printf 'PAIRED\tCLIENT_NAME\n' ;; *) exit 1 ;; esac
"#
    .replace("CREDENTIAL_FILE", CREDENTIAL_FILE)
    .replace("SUNSHINE_HTTPS_PORT", &SUNSHINE_HTTPS_PORT.to_string())
    .replace("CLIENT_NAME", client_name)
    .replace("PIN", pin);
    let output = host_channel::run_script(target, &script, runner).await?;
    let mut body = report(target, &output, "paired");
    if let Some(map) = body.as_object_mut() {
        map.insert(
            "fields".to_string(),
            Value::Object(parse_fields(&output.stdout)),
        );
    }
    Ok(body)
}

/// Stop the session. `purge` also removes the units and the Xorg screen, so a
/// host can go back to being headless without a trace beyond the packages.
pub async fn stop(
    target: &ComputeTarget,
    purge: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let purge_block = if purge {
        r#"systemctl disable SUNSHINE_UNIT XORG_UNIT >/dev/null 2>&1 || true
rm -f /etc/systemd/system/SUNSHINE_UNIT /etc/systemd/system/XORG_UNIT XORG_CONFIG
systemctl daemon-reload
# The library bind is this feature's too, so purge owns undoing it. Only the
# tagged line is touched: an operator's own mount at the same point stays.
if grep -q '# stado-stream$' /etc/fstab; then
  cp -p /etc/fstab "/etc/fstab.before-stream-purge-$(date -u +%Y%m%d)"
  grep -v '# stado-stream$' /etc/fstab >/etc/fstab.stado-stream-new
  mv /etc/fstab.stado-stream-new /etc/fstab
  printf 'FSTAB\tremoved the tagged library line\n'
fi
if awk -v point=LIBRARY_DIR '$2 == point { found = 1 } END { exit !found }' /proc/self/mounts; then
  umount LIBRARY_DIR && printf 'UNMOUNTED\tLIBRARY_DIR\n'
fi
printf 'PURGED\tunits and screen configuration removed\n'
"#
    } else {
        "printf 'KEPT\\tunits remain installed and enabled\\n'\n"
    };
    let script = format!(
        r#"set -euo pipefail
# The report carries stdout only, so a script whose error goes to stderr fails
# invisibly. Fold the two together: a host operation that breaks must say why.
exec 2>&1
systemctl stop SUNSHINE_UNIT >/dev/null 2>&1 || true
systemctl stop XORG_UNIT >/dev/null 2>&1 || true
{purge_block}printf 'XORG\t%s\n' "$(systemctl is-active XORG_UNIT 2>&1 || true)"
printf 'SUNSHINE\t%s\n' "$(systemctl is-active SUNSHINE_UNIT 2>&1 || true)"
"#
    )
    .replace("XORG_UNIT", XORG_UNIT)
    .replace("SUNSHINE_UNIT", SUNSHINE_UNIT)
    .replace("XORG_CONFIG", XORG_CONFIG)
    .replace("LIBRARY_DIR", &library_dir(target));
    let output = host_channel::run_script(target, &script, runner).await?;
    let mut body = report(target, &output, "stopped");
    if let Some(map) = body.as_object_mut() {
        map.insert(
            "fields".to_string(),
            Value::Object(parse_fields(&output.stdout)),
        );
    }
    Ok(body)
}

/// The board a probe reports for one driver UUID, as its PCI bus id.
pub fn bus_id_for(probe_report: &Value, gpu_uuid: Option<&str>) -> Option<String> {
    let boards = probe_report
        .get("fields")
        .and_then(|fields| fields.get("board"))?;
    let rows: Vec<String> = match boards {
        Value::String(single) => vec![single.clone()],
        Value::Array(list) => list
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    for row in rows {
        // index, uuid, name, pci.bus_id, memory.total
        let columns: Vec<&str> = row.split(',').map(str::trim).collect();
        let (Some(uuid), Some(bus)) = (columns.get(1), columns.get(3)) else {
            continue;
        };
        match gpu_uuid {
            Some(wanted) if *uuid != wanted => continue,
            _ => return xorg_bus_id(bus),
        }
    }
    None
}

/// nvidia-smi's `00000000:C2:00.0` as Xorg's `PCI:194:0:0`.
///
/// Xorg wants decimal, nvidia-smi prints hex, and passing the hex form through
/// verbatim produced a config with no matching device: the X server exited with
/// "no screens found" and systemd restarted it every five seconds, which from
/// outside looked like a unit stuck in `activating`.
pub fn xorg_bus_id(smi_bus_id: &str) -> Option<String> {
    let parts: Vec<&str> = smi_bus_id.split(':').collect();
    let (bus, tail) = match parts.as_slice() {
        [_domain, bus, tail] => (*bus, *tail),
        [bus, tail] => (*bus, *tail),
        _ => return None,
    };
    let (device, function) = tail.split_once('.')?;
    let bus = u32::from_str_radix(bus.trim(), 16).ok()?;
    let device = u32::from_str_radix(device.trim(), 16).ok()?;
    let function = u32::from_str_radix(function.trim(), 16).ok()?;
    Some(format!("PCI:{bus}:{device}:{function}"))
}

/// The declaration a fresh `stream declare` writes.
///
/// `release` is the host's own `ID VERSION_ID` from the probe, because the
/// artifact that installs is a property of the distribution and not of this
/// build: the 26.04 package wants `libc6 >= 2.43` and `libicu78`, and on the
/// fleet's Ubuntu 25.10 host apt answered "[no choices]" for exactly that
/// reason.
pub fn default_declaration(
    resolution: &str,
    refresh_hz: u16,
    gpu_uuid: Option<String>,
    library_dir: &str,
    steam: bool,
    release: &str,
) -> Result<DisplayStream, String> {
    Ok(DisplayStream {
        enabled: true,
        session: crate::stream::schema::SESSION_X11.to_string(),
        resolution: resolution.to_string(),
        refresh_hz,
        gpu_uuid,
        library_dir: library_dir.to_string(),
        sunshine: pinned_sunshine_for(release)?,
        steam,
    })
}

/// The Sunshine release this build pins, and which published artifact suits a
/// given distribution. Nothing here resolves "latest", and every digest was
/// measured from the published asset (the project's release API reports the same
/// values under `assets[].digest`).
pub const SUNSHINE_VERSION: &str = "v2026.516.143833";

const SUNSHINE_ARTIFACTS: &[(&str, &str, &str)] = &[
    // release prefix, asset name, sha256
    (
        "ubuntu 22.04",
        "sunshine-ubuntu-22.04-amd64.deb",
        "",
    ),
    (
        "ubuntu 24.04",
        "sunshine-ubuntu-24.04-amd64.deb",
        "6df8900f23c9c056252eea51639507b8239a1d1241308ab8923cb402b0ca653b",
    ),
    (
        // 25.10 carries libicu76 and glibc 2.42: the Debian trixie build is the
        // published artifact whose dependencies that satisfies, while both the
        // 24.04 (libicu74) and 26.04 (libicu78, glibc 2.43) packages do not.
        "ubuntu 25.10",
        "sunshine-debian-trixie-amd64.deb",
        "b9b65f2be93b3e30be0710a940a616b1381da5bc6d858dce33bc0094d7fd4131",
    ),
    (
        "ubuntu 26.04",
        "sunshine-ubuntu-26.04-amd64.deb",
        "c7e5452f8cf2609dffbdeda63ca3be7ee45f91505dc496844d65924817cb2517",
    ),
    (
        "debian 13",
        "sunshine-debian-trixie-amd64.deb",
        "b9b65f2be93b3e30be0710a940a616b1381da5bc6d858dce33bc0094d7fd4131",
    ),
];

/// The pinned artifact for one distribution, or a refusal that names what is
/// known. A guess here would be a host that installs something its libraries
/// cannot load.
pub fn pinned_sunshine_for(release: &str) -> Result<crate::stream::schema::SunshineRelease, String> {
    let normalised = release.trim().to_lowercase();
    let found = SUNSHINE_ARTIFACTS
        .iter()
        .find(|(prefix, _, digest)| normalised.starts_with(prefix) && !digest.is_empty());
    let Some((_, asset, digest)) = found else {
        let known: Vec<&str> = SUNSHINE_ARTIFACTS
            .iter()
            .filter(|(_, _, digest)| !digest.is_empty())
            .map(|(prefix, _, _)| *prefix)
            .collect();
        return Err(format!(
            "no Sunshine artifact is pinned for {release:?}; known: {}. Pass --sunshine-url and \
             --sunshine-sha256 for a measured artifact instead of guessing",
            known.join(", ")
        ));
    };
    Ok(crate::stream::schema::SunshineRelease {
        version: SUNSHINE_VERSION.to_string(),
        deb_url: format!(
            "https://github.com/LizardByte/Sunshine/releases/download/{SUNSHINE_VERSION}/{asset}"
        ),
        deb_sha256: (*digest).to_string(),
    })
}

/// Report shaped for `--json`, with the declaration echoed beside the host's
/// answer so a reader sees both halves of the comparison.
pub fn with_declaration(mut report: Value, declaration: &DisplayStream) -> Value {
    if let Some(map) = report.as_object_mut() {
        map.insert(
            "declaration".to_string(),
            serde_json::to_value(declaration).unwrap_or(json!(null)),
        );
    }
    report
}
