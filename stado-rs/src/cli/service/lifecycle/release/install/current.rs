//! Moving a unit onto the `current` symlink, and proving the unit it wrote
//! can be executed.

use super::*;

/// Make the unit execute through `current`, if it was rendered against a
/// version directory instead.
///
/// The program is taken from the declaration, never from the rendered unit
/// summary. That summary is the program AND its arguments in one string, so
/// asking it for a path filename answers "stado coordinator" for the
/// control-plane unit and writes that as the program, leaving the declared
/// `coordinator` to be
/// appended a second time: the unit file came out as
/// `.../current/<platform>/stado coordinator coordinator`, an argv the binary
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
    // can drift apart -- the declaration naming the package program and the
    // file saying `stado coordinator coordinator` -- and an earlier
    // version of this function returned here without looking, because the
    // DECLARED path was already on `current` and there was seemingly nothing
    // to repoint. Nothing else in the deployment path compares the two, so
    // that file would have sat there until the next reload.
    let report = service::show_service(target, declared, runner)
        .await
        .map_err(CmdError::from)?;
    // The summary is `<program> <args...>`, followed by ` (current -> ...)`
    // when the unit runs through a `current` link.
    let detail = report.detail.trim();
    let rendered = match detail.split_once(" (current ->") {
        Some((summary, _)) => summary.trim(),
        None => detail,
    }
    .to_string();
    // A declaration that renders the unit states its program on its own. Only
    // a declaration that merely points at a hand-installed plist has none, and
    // then the rendered summary is the only source there is -- read for its
    // path alone rather than for a filename that would carry the arguments
    // with it. A summary that names no program either is refused.
    let program = if declared.program.trim().is_empty() {
        rendered.split_whitespace().next().ok_or_else(|| {
            CmdError::click(format!(
                "{} declares no program and its unit on {} names none, so there is nothing to \
                 point at current",
                declared.unit_id(),
                target.name
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?
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
    // The archive installer lays a release out below the target's own
    // platform directory, the one the catalog's `$STADO_PLATFORM` names.
    let platform_dir = crate::deploy::service_catalog::platform_directory(&target.release_platform);
    let wanted = if let Some((root, below)) =
        super::program_under_current(program, directory, platform_dir)
    {
        let on_current = format!("{root}current/{below}");
        if on_current == program {
            program.to_string()
        } else {
            on_current
        }
    } else if program.contains(&marker) {
        return Ok(false);
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
                .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        format!(".stado/services/{directory}/current/{platform_dir}/{executable}")
    };
    // The declared arguments that must follow the program, in the
    // declaration's own words and order. Only the tail travels: the program
    // itself is `$wanted`, which the remote side is what expands a
    // `$HOME`-relative path, so composing the expected vector there is the
    // only way the comparison sees the same absolute path the plist got.
    // Empty program means a declaration that only names a plist, and then
    // there is no declared vector to compare against.
    let (compare_argv, expected_args) = if declared.program.trim().is_empty() {
        ("no", String::new())
    } else {
        ("yes", declared.args.join("\n"))
    };
    let script = format!(
        "set -euo pipefail\nunit_path={}\nwanted={}\ncompare_argv={compare_argv}\nexpected_args={}\n{REPOINT_BODY}",
        crate::deploy::shlex_quote(&declared.path),
        crate::deploy::shlex_quote(&wanted),
        crate::deploy::shlex_quote(&expected_args),
    );
    let output = host_channel::run_script(target, &script, runner)
        .await
        .map_err(CmdError::from)?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: installed the version but could not point {} at current: {}",
            target.name,
            declared.unit_id(),
            host_channel::last_error_line(&output, "repoint failed")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(true)
}

/// Repoint one unit's program, and refuse to leave behind a unit that cannot
/// be executed.
///
/// A loaded service can keep running even after its unit file becomes
/// unusable. Verify the persisted program is executable and its argument
/// vector exactly matches the declaration; restore the previous file and
/// return failure if either check fails. `false; exit` leaves with the
/// failing status of `false`, inside `||` lists where `set -e` does not act.
const REPOINT_BODY: &str = r#"
[ -f "$unit_path" ] || { printf '%s\n' "no unit file at $unit_path" >&2; false; exit; }
case "$unit_path" in
  /Library/*) sudo_prefix="/usr/bin/sudo -n" ;;
  *) sudo_prefix="" ;;
esac
case "$wanted" in
  /*) ;;
  *) wanted="$HOME/$wanted" ;;
esac
plist() { $sudo_prefix /usr/libexec/PlistBuddy -c "$1" "$unit_path"; }
has_key() { plist "Print :$1" &>/dev/null; }
backup="$(/usr/bin/mktemp)"
$sudo_prefix /bin/cat "$unit_path" > "$backup"
restore_and_fail() {
  $sudo_prefix /bin/cp "$backup" "$unit_path"
  rm -f "$backup"
  printf '%s\n' "$1" >&2
  false
  exit
}
if [ "$compare_argv" = yes ]; then
  # The whole vector is rewritten from the declaration, not just its first
  # element. Setting element zero alone is what let a duplicated subcommand
  # survive a repoint: the program was corrected in place while the stale
  # extra word sat behind it, and the file stayed unrunnable.
  if has_key ProgramArguments; then
    plist "Delete :ProgramArguments" || restore_and_fail "could not clear ProgramArguments on $unit_path"
  fi
  plist "Add :ProgramArguments array" || restore_and_fail "could not create ProgramArguments on $unit_path"
  plist "Add :ProgramArguments: string $wanted" || restore_and_fail "could not set the program on $unit_path"
  if [ -n "$expected_args" ]; then
    while IFS= read -r word; do
      [ -n "$word" ] || continue
      plist "Add :ProgramArguments: string $word" || restore_and_fail "could not set the declared argument $word on $unit_path"
    done <<< "$expected_args"
  fi
  if has_key Program; then
    plist "Delete :Program" || restore_and_fail "could not clear Program on $unit_path"
  fi
elif has_key ProgramArguments; then
  # A launchd job runs the first ProgramArguments entry, which PlistBuddy
  # addresses by its index (https://keith.github.io/xcode-man-pages/launchd.plist.5.html).
  plist "Set :ProgramArguments:0 $wanted" || restore_and_fail "could not set the program on $unit_path"
else
  plist "Set :Program $wanted" || restore_and_fail "could not set the program on $unit_path"
fi
if has_key ProgramArguments; then
  rendered="$(plist 'Print :ProgramArguments' | /usr/bin/grep -v -e '^Array {$' -e '^}$' | /usr/bin/sed -e 's/^ *//' | /usr/bin/grep -v '^$')"
else
  rendered="$(plist 'Print :Program')"
fi
IFS= read -r program <<< "$rendered"
[ -f "$program" ] || restore_and_fail "the unit would run $program, which is not a file on this host"
[ -x "$program" ] || restore_and_fail "the unit would run $program, which is not executable"
if [ "$compare_argv" = yes ]; then
  if [ -n "$expected_args" ]; then
    expected_argv="$(printf '%s\n%s' "$wanted" "$expected_args")"
  else
    expected_argv="$wanted"
  fi
  if [ "$rendered" != "$expected_argv" ]; then
    restore_and_fail "the rendered argument vector [$(printf '%s' "$rendered" | /usr/bin/tr '\n' ' ')] does not match the declared one [$(printf '%s' "$expected_argv" | /usr/bin/tr '\n' ' ')]"
  fi
fi
rm -f "$backup"
printf '%s\n' "$wanted"
"#;
