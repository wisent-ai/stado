//! Stopping the pair, and the purge that takes the host back to headless.

use serde_json::Value;

use crate::deploy::stream::{
    library_dir, parse_fields, report, SUNSHINE_UNIT, XORG_CONFIG, XORG_UNIT,
};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

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
