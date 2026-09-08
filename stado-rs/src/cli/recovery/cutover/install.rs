//! Installing the prepared bytes, atomically, on this host and on every host.
//!
//! Both halves follow the same shape: keep a one-time `.pre-stado-recovery`
//! rollback copy, write a private temporary file beside the target, then
//! rename it into place. The remote half is a base64-carried program so the
//! path and the document never pass through shell quoting, and it reports a
//! marker line the caller insists on seeing before it believes the write.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::cli::recovery::deploy_error;
use crate::cli::recovery::request::{PreparedConfig, ResolvedService};
use crate::cli::CmdError;
use crate::deploy::{host_channel, production_runner};
use crate::targets::ComputeTarget;

pub(in crate::cli::recovery) fn install_local_config(
    prepared: &PreparedConfig,
) -> Result<(), CmdError> {
    let parent = prepared.path.parent().ok_or_else(|| {
        CmdError::click(format!(
            "{} has no parent directory",
            prepared.path.display()
        ))
    })?;
    fs::create_dir_all(parent)?;
    let backup = path_with_suffix(&prepared.path, ".pre-stado-recovery")?;
    if prepared.path.exists() && !backup.exists() {
        fs::copy(&prepared.path, &backup)?;
    }
    let temp = parent.join(format!(
        ".{}.recovery-{}.tmp",
        prepared
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stado-config"),
        uuid::Uuid::new_v4()
    ));
    let write_result = (|| -> Result<(), std::io::Error> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&prepared.bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &prepared.path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result?;
    println!(
        "  wrote {} (rollback: {})",
        prepared.path.display(),
        backup.display()
    );
    Ok(())
}

fn path_with_suffix(path: &Path, suffix: &str) -> Result<PathBuf, CmdError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CmdError::click(format!("invalid config path {}", path.display())))?;
    Ok(path.with_file_name(format!("{name}{suffix}")))
}

pub(in crate::cli::recovery) async fn install_remote_configs(
    services: &[ResolvedService],
    bytes: &[u8],
) -> Result<(), CmdError> {
    let mut destinations: BTreeMap<(String, String), ComputeTarget> = BTreeMap::new();
    for resolved in services {
        destinations.insert(
            (
                resolved.reference.host.clone(),
                resolved.config_path.clone(),
            ),
            resolved.target.clone(),
        );
    }
    let runner = production_runner();
    for ((host, path), target) in destinations {
        let script = remote_config_script(&path, bytes);
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(deploy_error)?;
        if !output.ok() || !output.stdout.contains("STADO_RECOVERY_CONFIG\tinstalled\t") {
            return Err(CmdError::click(format!(
                "{host}: config cutover failed: {}",
                host_channel::last_error_line(&output, "missing recovery config marker")
            )));
        }
        println!("  {host}: wrote {path}");
    }
    Ok(())
}

fn remote_config_script(path: &str, bytes: &[u8]) -> String {
    let path_b64 = STANDARD.encode(path.as_bytes());
    let body_b64 = STANDARD.encode(bytes);
    format!(
        r#"set -eu
umask 077
case "$(/usr/bin/uname -s)" in
  Darwin) decode_flag=-D ;;
  *) decode_flag=--decode ;;
esac
config_path=$(printf '%s' '{path_b64}' | /usr/bin/base64 "$decode_flag")
case "$config_path" in
  /*) ;;
  *) printf 'config path is not absolute\n' >&2; exit 64 ;;
esac
parent=$(/usr/bin/dirname "$config_path")
/bin/mkdir -p "$parent"
tmp="$config_path.recovery.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
printf '%s' '{body_b64}' | /usr/bin/base64 "$decode_flag" > "$tmp"
/bin/chmod 600 "$tmp"
if [ -f "$config_path" ] && [ ! -f "$config_path.pre-stado-recovery" ]; then
  /bin/cp -p "$config_path" "$config_path.pre-stado-recovery"
fi
/bin/mv -f "$tmp" "$config_path"
trap - EXIT HUP INT TERM
printf 'STADO_RECOVERY_CONFIG\tinstalled\t%s\n' "$config_path"
"#
    )
}
