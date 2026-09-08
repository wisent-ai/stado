//! The read-only question a host answers before anything is installed, and the
//! board address its answer yields.

use serde_json::Value;

use crate::deploy::stream::{library_dir, parse_fields, report, SUNSHINE_UNIT, XORG_UNIT};
use crate::deploy::{host_channel, DeployError, Runner};
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
