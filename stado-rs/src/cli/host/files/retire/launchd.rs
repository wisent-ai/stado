use std::ffi::OsStr;
use std::path::Path;

use crate::cli::CmdError;

use crate::cli::host::files::retire::{
    retire_refused, safe_backup_product, RetireFileBinding, RetireFileOutcome, RetireFileRequest,
};

const RETIRE_SYSTEM_LAUNCHD_FILE: &str = r#"set -eu
src=$1
dst=$2
dry=$3
expected_sha=$4
expected_size=$5
expected_mode=$6
if [ ! -e "$src" ]; then
  printf 'STADO_RETIRE_SYSTEM\tabsent\t-\t-\t-\t-\n'
  exit 0
fi
[ -f "$src" ] && [ ! -L "$src" ] || {
  printf 'source is not a regular non-symlink file\n' >&2
  exit 65
}
owner=$(/usr/bin/stat -f '%u' "$src")
[ "$owner" = 0 ] || {
  printf 'source is not owned by root\n' >&2
  exit 65
}
size=$(/usr/bin/stat -f '%z' "$src")
mode=$(/usr/bin/stat -f '%Lp' "$src")
case ${#mode} in 3) mode=0$mode ;; esac
sha=$(/usr/bin/shasum -a 256 "$src" | /usr/bin/awk '{print $1}')
if [ "$expected_sha" != - ]; then
  [ "$sha" = "$expected_sha" ] &&
  [ "$size" = "$expected_size" ] &&
  [ "$mode" = "$expected_mode" ] || {
    printf 'source differs from the reviewed dry-run receipt\n' >&2
    exit 65
  }
fi
if [ "$dry" = yes ]; then
  printf 'STADO_RETIRE_SYSTEM\tready\t%s\t%s\t%s\t%s\n' "$size" "$sha" "$mode" "$dst"
  exit 0
fi
[ ! -e "$dst" ] || {
  printf 'destination collision\n' >&2
  exit 65
}
/bin/mv -n "$src" "$dst"
if [ -e "$src" ] || [ ! -f "$dst" ] || [ -L "$dst" ]; then
  [ ! -e "$src" ] && [ -e "$dst" ] && /bin/mv -n "$dst" "$src" || true
  printf 'retirement path postcondition failed\n' >&2
  exit 65
fi
after_size=$(/usr/bin/stat -f '%z' "$dst")
after_mode=$(/usr/bin/stat -f '%Lp' "$dst")
case ${#after_mode} in 3) after_mode=0$after_mode ;; esac
after_sha=$(/usr/bin/shasum -a 256 "$dst" | /usr/bin/awk '{print $1}')
if [ "$after_size" != "$size" ] || [ "$after_mode" != "$mode" ] || [ "$after_sha" != "$sha" ]; then
  /bin/mv -n "$dst" "$src" || true
  printf 'retired file digest, size, or mode changed; source restored\n' >&2
  exit 65
fi
printf 'STADO_RETIRE_SYSTEM\tretired\t%s\t%s\t%s\t%s\n' "$size" "$sha" "$mode" "$dst"
"#;

pub(super) async fn retire_system_launchd_file(
    target: &crate::targets::ComputeTarget,
    request: &RetireFileRequest<'_>,
    binding: Option<&RetireFileBinding>,
) -> Result<RetireFileOutcome, CmdError> {
    if !target.release_platform.starts_with("darwin-") {
        return Err(retire_refused(
            "system launchd retirement requires a Darwin target",
        ));
    }
    let source = Path::new(request.path);
    if source.parent() != Some(Path::new("/Library/LaunchDaemons"))
        || source.extension().and_then(OsStr::to_str) != Some("plist")
    {
        return Err(retire_refused(
            "privileged retirement is limited to one /Library/LaunchDaemons/*.plist file",
        ));
    }
    let name = source
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| retire_refused("system launchd source has no UTF-8 basename"))?;
    let stem = name
        .strip_suffix(".plist")
        .ok_or_else(|| retire_refused("system launchd source must end in .plist"))?;
    if stem.is_empty()
        || !stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(retire_refused(
            "system launchd basename contains unsupported characters",
        ));
    }
    if !safe_backup_product(request.product) {
        return Err(CmdError::usage(
            "product must be 1-128 ASCII letters, digits, dots, underscores, or dashes and start with a letter or digit",
        ));
    }
    let transaction = binding
        .map(|binding| binding.transaction.clone())
        .unwrap_or_else(|| {
            format!(
                "{}-{}",
                chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
                uuid::Uuid::new_v4().simple()
            )
        });
    let destination = format!("/Library/LaunchDaemons/{name}.stado-retired-{transaction}");
    let expected_size = binding
        .map(|binding| binding.expected_size.to_string())
        .unwrap_or_else(|| "-".to_string());
    let expected_sha = binding
        .map(|binding| binding.expected_sha256.as_str())
        .unwrap_or("-");
    let expected_mode = binding
        .map(|binding| binding.expected_mode.as_str())
        .unwrap_or("-");
    let password = crate::cli::service::host_sudo_password(target)
        .await?
        .unwrap_or_default();
    let output = crate::deploy::host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/sudo",
            "-S",
            "-p",
            "",
            "/bin/sh",
            "-c",
            RETIRE_SYSTEM_LAUNCHD_FILE,
            "stado-retire-system-launchd",
            request.path,
            &destination,
            if request.dry_run { "yes" } else { "no" },
            expected_sha,
            &expected_size,
            expected_mode,
        ],
        &format!("{password}\n"),
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(retire_refused(format!(
            "{}: privileged launchd retirement failed: {}",
            target.name,
            crate::deploy::host_channel::last_error_line(&output, "remote command failed")
        )));
    }
    let marker = output
        .stdout
        .lines()
        .find(|line| line.starts_with("STADO_RETIRE_SYSTEM\t"))
        .ok_or_else(|| retire_refused("privileged launchd retirement returned no marker"))?;
    let fields: Vec<&str> = marker.split('\t').collect();
    if fields.len() != 6 {
        return Err(retire_refused(
            "privileged launchd retirement returned a malformed marker",
        ));
    }
    if fields[1] == "absent" {
        return Ok(RetireFileOutcome {
            target: target.name.clone(),
            source: request.path.to_string(),
            destination: None,
            transaction: None,
            status: "absent".to_string(),
            size: None,
            sha256: None,
            mode: None,
            detail: Some("source does not exist".to_string()),
        });
    }
    let expected_status = if request.dry_run { "ready" } else { "retired" };
    if fields[1] != expected_status {
        return Err(retire_refused(format!(
            "privileged launchd retirement returned status {:?}, expected {expected_status:?}",
            fields[1]
        )));
    }
    let size = fields[2]
        .parse::<u64>()
        .map_err(|_| retire_refused("privileged launchd retirement returned an invalid size"))?;
    if fields[3].len() != 64 || !fields[3].bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(retire_refused(
            "privileged launchd retirement returned an invalid SHA-256",
        ));
    }
    Ok(RetireFileOutcome {
        target: target.name.clone(),
        source: request.path.to_string(),
        destination: Some(fields[5].to_string()),
        transaction: Some(transaction),
        status: fields[1].to_string(),
        size: Some(size),
        sha256: Some(fields[3].to_string()),
        mode: Some(fields[4].to_string()),
        detail: None,
    })
}
