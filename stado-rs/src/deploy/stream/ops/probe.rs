//! The read-only question a host answers before anything is installed, and the
//! board address its answer yields.

use serde_json::Value;

use crate::deploy::stream::{library_dir, parse_fields, report, SUNSHINE_UNIT, XORG_UNIT};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::primitives::failure::FailureCode;
use crate::targets::ComputeTarget;

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
if [ -n "LIBRARY_DIR" ]; then
  printf 'LIBRARY_FREE_KIB\t'; df -Pk "LIBRARY_DIR" 2>/dev/null | awk 'NR==2 { print $4 }' || printf 'unknown\n'
else
  printf 'LIBRARY_FREE_KIB\tundeclared\n'
fi
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

/// Why a probe names no board to render on.
#[derive(Debug, PartialEq, Eq)]
pub enum BoardRefusal {
    /// The host reported its boards and none is the one wanted.
    NotFound(String),
    /// The probe's board report, one of its rows, or a row's bus id cannot be read.
    Unreadable(String),
}

impl BoardRefusal {
    /// The failure an operator sees: a missing board, or a host whose answer is unreadable.
    pub fn code(&self) -> FailureCode {
        match self {
            Self::NotFound(_) => FailureCode::NotFound,
            Self::Unreadable(_) => FailureCode::InfraDown,
        }
    }
}

impl std::fmt::Display for BoardRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(cause) | Self::Unreadable(cause) => f.write_str(cause),
        }
    }
}

/// The board a probe reports for one driver UUID (the first board when none is
/// named), as its Xorg PCI bus id. Each BOARD row holds the five columns the
/// probe asks nvidia-smi for; a row of any other shape, or a bus id that is not
/// one, is refused naming it, never skipped.
pub fn bus_id_for(probe_report: &Value, gpu_uuid: Option<&str>) -> Result<String, BoardRefusal> {
    let rows: Vec<&str> = match probe_report.get("fields").and_then(|fields| fields.get("board")) {
        None => Vec::new(),
        Some(Value::String(single)) => vec![single.as_str()],
        Some(Value::Array(list)) => list
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| BoardRefusal::Unreadable(format!("a BOARD entry of the probe is not text: {value}")))
            })
            .collect::<Result<_, _>>()?,
        Some(other) => {
            return Err(BoardRefusal::Unreadable(format!(
                "the probe's board field is neither a line nor a list: {other}"
            )))
        }
    };
    let mut reported = Vec::new();
    for row in rows {
        let columns: Vec<&str> = row.split(',').map(str::trim).collect();
        let [_index, uuid, _name, bus, _memory] = columns.as_slice() else {
            return Err(BoardRefusal::Unreadable(format!(
                "board row {row:?} is not index, uuid, name, pci.bus_id, memory.total"
            )));
        };
        match gpu_uuid {
            Some(wanted) if *uuid != wanted => reported.push(uuid.to_string()),
            _ => return xorg_bus_id(bus).map_err(|cause| BoardRefusal::Unreadable(format!("board {uuid}: {cause}"))),
        }
    }
    Err(BoardRefusal::NotFound(match gpu_uuid {
        Some(wanted) if reported.is_empty() => format!("no board with uuid {wanted}: the host reports no NVIDIA board"),
        Some(wanted) => format!("no board with uuid {wanted}; the host reports {}", reported.join(", ")),
        None => "the host reports no NVIDIA board".to_string(),
    }))
}

/// The digits nvidia-smi writes a PCI bus id in; their count is the radix.
const HEX_DIGITS: &str = "0123456789abcdef";

/// nvidia-smi's `00000000:C2:00.0` as Xorg's `PCI:194:0:0`.
///
/// Xorg wants decimal, nvidia-smi prints hex, and passing the hex form through
/// verbatim produced a config with no matching device: the X server exited with
/// "no screens found" and systemd restarted it every five seconds, which from
/// outside looked like a unit stuck in `activating`.
pub fn xorg_bus_id(smi_bus_id: &str) -> Result<String, String> {
    let parts: Vec<&str> = smi_bus_id.split(':').collect();
    let (bus, tail) = match parts.as_slice() {
        [_, bus, tail] | [bus, tail] => (*bus, *tail),
        _ => return Err(format!("PCI bus id {smi_bus_id:?} is not [domain:]bus:device.function")),
    };
    let (device, function) = tail
        .split_once('.')
        .ok_or_else(|| format!("PCI bus id {smi_bus_id:?} names no .function"))?;
    let radix = HEX_DIGITS.chars().count() as u32;
    let read = |field: &str, what: &str| {
        u32::from_str_radix(field.trim(), radix)
            .map_err(|error| format!("PCI bus id {smi_bus_id:?}: {what} {field:?} is not hex: {error}"))
    };
    let bus = read(bus, "bus")?;
    let device = read(device, "device")?;
    let function = read(function, "function")?;
    Ok(format!("PCI:{bus}:{device}:{function}"))
}
