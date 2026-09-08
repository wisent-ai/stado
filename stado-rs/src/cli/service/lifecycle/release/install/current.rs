//! Moving a unit onto the `current` symlink, and proving the unit it wrote
//! can be executed.

use super::*;

/// Make the unit execute through `current`, if it was rendered against a
/// version directory instead.
///
/// The program is taken from the declaration, never from the rendered unit
/// summary. That summary is the program AND its arguments in one string, so
/// asking it for a path filename answered "stado coordinator" for
/// `com.wisent.compute.service.stado-local-control-plane` on 2026-09-03 and
/// wrote that as the program, leaving the declared `coordinator` to be
/// appended a second time: the unit file came out as
/// `.../current/darwin-arm/stado coordinator coordinator`, an argv the binary
/// cannot parse. launchd happened to still hold the previous job, so the
/// coordinator kept dispatching and the mis-render waited for the next time
/// anything made launchd re-read that plist.
///
/// It never showed on a unit already running from `current`, because the
/// marker branch below returns before any of this. It fires on exactly the
/// units being MOVED onto a content-addressed package -- the operation this
/// function exists for.
///
/// Returns whether anything had to change.
pub(crate) async fn follow_current(
    target: &crate::targets::ComputeTarget,
    declared: &crate::deploy::service::ManagedService,
    directory: &str,
    runner: &crate::deploy::Runner,
) -> Result<bool, CmdError> {
    // What the unit file on the host actually says today, always read, because
    // the declaration being right is not evidence that the file is. Those two
    // drifted apart on 2026-09-03 -- the declaration named the package program
    // and the file said `stado coordinator coordinator` -- and an earlier
    // version of this function returned here without looking, because the
    // DECLARED path was already on `current` and there was seemingly nothing
    // to repoint. Nothing else in the deployment path compares the two, so
    // that file would have sat there until the next reload.
    let report = service::show_service(target, declared, runner)
        .await
        .map_err(click)?;
    // The summary is `<program> <args...>`, then ` (current -> ...)`.
    let rendered = report
        .detail
        .trim()
        .split(" (current ->")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    // A declaration that renders the unit states its program on its own. Only
    // a declaration that merely points at a hand-installed plist has none, and
    // then the rendered summary is the only source there is -- read for its
    // path alone rather than for a filename that would carry the arguments
    // with it.
    let program = if declared.program.trim().is_empty() {
        rendered.split_whitespace().next().unwrap_or_default()
    } else {
        declared.program.trim()
    };
    let marker = format!("/services/{directory}/");
    let pinned_to_a_version = match program.split_once(&marker) {
        Some((_, rest)) => match rest.split_once('/') {
            Some((segment, _)) => segment != "current",
            None => false,
        },
        None => true,
    };
    // The file agrees with the declaration word for word and the declaration
    // is already on `current`: nothing to write.
    let declared_argv = if declared.program.trim().is_empty() {
        String::new()
    } else {
        let mut words = vec![program.to_string()];
        words.extend(declared.args.iter().cloned());
        words.join(" ")
    };
    if !pinned_to_a_version && (declared_argv.is_empty() || declared_argv == rendered) {
        return Ok(false);
    }
    let wanted = if let Some((root, rest)) = program.split_once(&marker) {
        let Some((segment, tail)) = rest.split_once('/') else {
            return Ok(false);
        };
        if segment == "current" {
            program.to_string()
        } else {
            format!("{root}{marker}current/{tail}")
        }
    } else {
        let executable = std::path::Path::new(program)
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{} runs {program:?}, which has no executable filename",
                    declared.unit_id()
                ))
            })?;
        format!(".stado/services/{directory}/current/darwin-arm/{executable}")
    };
    // The declared arguments that must follow the program, in the
    // declaration's own words and order. Only the tail travels: the program
    // itself is `$wanted`, which the remote side is what expands a
    // `$HOME`-relative path, so composing the expected vector there is the
    // only way the comparison sees the same absolute path the plist got.
    // Empty program means a declaration that only names a plist, and then
    // there is no declared vector to compare against.
    let expected_args = if declared.program.trim().is_empty() {
        String::new()
    } else {
        declared.args.join("\n")
    };
    let script = format!(
        "set -euo pipefail\nunit_path={}\nwanted={}\ncompare_argv={}\nexpected_args={}\n{REPOINT_BODY}",
        crate::deploy::shlex_quote(&declared.path),
        crate::deploy::shlex_quote(&wanted),
        if declared.program.trim().is_empty() {
            "0"
        } else {
            "1"
        },
        crate::deploy::shlex_quote(&expected_args),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(click)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: installed the version but could not point {} at current: {}",
            target.name,
            declared.unit_id(),
            host_channel::last_error_line(&output, "repoint failed")
        )));
    }
    Ok(true)
}

/// Repoint one unit's program, and refuse to leave behind a unit that cannot
/// be executed.
///
/// The write is checked because an unchecked one has already happened: on
/// 2026-09-03 this step rendered
/// `.../current/darwin-arm/stado coordinator coordinator` for the fleet's
/// coordinator, wrote it, and reported success. Nothing compared what it had
/// written against anything, so the only reason the fleet kept dispatching is
/// that launchd was still holding the previous job in memory; the file would
/// have taken effect at the next boot, bootout or reload, with no one left to
/// unwind it. A step that writes a unit now proves the unit it wrote: the
/// program is an existing executable file on this host, and the rendered
/// argument vector equals the declared one word for word. Either check
/// failing restores the file it found and exits non-zero, so the caller gets
/// a refusal instead of a landmine.
const REPOINT_BODY: &str = r#"
[ -f "$unit_path" ] || { printf '%s
' "no unit file at $unit_path" >&2; exit 1; }
case "$unit_path" in
  /Library/*) sudo_prefix="/usr/bin/sudo -n" ;;
  *) sudo_prefix="" ;;
esac
case "$wanted" in
  /*) ;;
  *) wanted="$HOME/$wanted" ;;
esac
backup="$(/usr/bin/mktemp)"
$sudo_prefix /bin/cat "$unit_path" > "$backup"
restore_and_fail() {
  $sudo_prefix /bin/cp "$backup" "$unit_path"
  rm -f "$backup"
  printf '%s
' "$1" >&2
  exit 1
}
if [ "${compare_argv:-0}" = "1" ]; then
  # The whole vector is rewritten from the declaration, not just its first
  # element. Setting element zero alone is what let a duplicated subcommand
  # survive a repoint: the program was corrected in place while the stale
  # extra word sat behind it, and the file stayed unrunnable.
  $sudo_prefix /usr/libexec/PlistBuddy -c "Delete :ProgramArguments" "$unit_path" >/dev/null 2>&1 || true
  $sudo_prefix /usr/libexec/PlistBuddy -c "Add :ProgramArguments array" "$unit_path"     || restore_and_fail "could not create ProgramArguments on $unit_path"
  $sudo_prefix /usr/libexec/PlistBuddy -c "Add :ProgramArguments: string $wanted" "$unit_path"     || restore_and_fail "could not set the program on $unit_path"
  if [ -n "${expected_args:-}" ]; then
    printf '%s\n' "$expected_args" | while IFS= read -r word; do
      [ -n "$word" ] || continue
      $sudo_prefix /usr/libexec/PlistBuddy -c "Add :ProgramArguments: string $word" "$unit_path" || exit 1
    done || restore_and_fail "could not set the declared arguments on $unit_path"
  fi
  $sudo_prefix /usr/libexec/PlistBuddy -c "Delete :Program" "$unit_path" >/dev/null 2>&1 || true
else
  $sudo_prefix /usr/libexec/PlistBuddy -c "Set :ProgramArguments:0 $wanted" "$unit_path"     || $sudo_prefix /usr/libexec/PlistBuddy -c "Set :Program $wanted" "$unit_path"     || restore_and_fail "could not set the program on $unit_path"
fi
rendered="$($sudo_prefix /usr/libexec/PlistBuddy -c 'Print :ProgramArguments' "$unit_path" 2>/dev/null | /usr/bin/sed -e '1d' -e '$d' -e 's/^ *//' | /usr/bin/grep -v '^$')"
[ -n "$rendered" ] || rendered="$($sudo_prefix /usr/libexec/PlistBuddy -c 'Print :Program' "$unit_path" 2>/dev/null)"
program="$(printf '%s
' "$rendered" | /usr/bin/sed -n '1p')"
[ -f "$program" ] || restore_and_fail "the unit would run $program, which is not a file on this host"
[ -x "$program" ] || restore_and_fail "the unit would run $program, which is not executable"
if [ "${compare_argv:-0}" = "1" ]; then
  if [ -n "${expected_args:-}" ]; then
    expected_argv="$(printf '%s\n%s' "$wanted" "$expected_args")"
  else
    expected_argv="$wanted"
  fi
  if [ "$rendered" != "$expected_argv" ]; then
    restore_and_fail "the rendered argument vector [$(printf '%s' "$rendered" | /usr/bin/tr '
' ' ')] does not match the declared one [$(printf '%s' "$expected_argv" | /usr/bin/tr '
' ' ')]"
  fi
fi
rm -f "$backup"
printf '%s
' "$wanted"
"#;
