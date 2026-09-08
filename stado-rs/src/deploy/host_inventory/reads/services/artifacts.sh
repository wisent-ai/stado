# The artefacts behind the service units, which no version report has ever
# looked at. `service converge` compares the DECLARED products under
# `$HOME/<root>/<name>` against the registry and says `in-sync`; a unit whose
# program is `$HOME/.stado/services/<label>/current/<platform>/<program>` is
# versioned by content hash and was swept by nothing. On 2026-08-31
# charless-mac-mini's object API - the store behind `stado://probierz` and
# the release ingress - was serving an artefact from before 19 August under a
# current declaration, on a host whose `.stado/bin/stado` had been 0.13.13
# since that morning. Both mtimes travel here so the comparison is a fact
# rather than an inference: an artefact older than the installed program of
# the same name is running code the fleet has already replaced.
printf '],"service_artifacts":['
separator=""
services_root="$HOME/.stado/services"
if [ -d "$services_root" ]; then
  for service_dir in "$services_root"/*; do
    [ -d "$service_dir" ] || continue
    label=${service_dir##*/}
    link="$service_dir/current"
    [ -L "$link" ] || continue
    target=$(/usr/bin/readlink "$link" 2>/dev/null || true)
    artefact_epoch=""
    program_name=""
    resolved_program=""
    installed_epoch=""
    resolved="$link"
    case "$target" in
      /*) resolved="$target" ;;
      ?*) resolved="$service_dir/$target" ;;
    esac
    if [ -d "$resolved" ]; then
      # The one executable file the platform directory holds, at the depth
      # every service release writes it. No -exec, no walk of unknown depth.
      for candidate in "$resolved"/*/* "$resolved"/*; do
        [ -f "$candidate" ] && [ -x "$candidate" ] || continue
        program_name=${candidate##*/}
        resolved_program="$candidate"
        artefact_epoch=$(/usr/bin/stat -f %m "$candidate" 2>/dev/null || true)
        break
      done
    fi
    # An mtime says when the file was written, never what is inside it: a
    # restaged old release is newer on disk and older in behaviour. The
    # version is the fact that settles it, asked the same way the binaries
    # loop above asks - one plain line, and only of a program this fleet
    # declares, so nothing here executes a file the product does not own.
    artefact_version=""
    installed_version=""
    case "$program_name" in
      stado|skarbiec-cli|brama)
        if [ -n "$artefact_epoch" ]; then
          artefact_version=$("$resolved_program" --version 2>/dev/null | /usr/bin/head -n 1 || true)
        fi
        ;;
    esac
    if [ -n "$program_name" ] && [ -f "$HOME/.stado/bin/$program_name" ]; then
      installed_epoch=$(/usr/bin/stat -f %m "$HOME/.stado/bin/$program_name" 2>/dev/null || true)
      case "$program_name" in
        stado|skarbiec-cli|brama)
          installed_version=$("$HOME/.stado/bin/$program_name" --version 2>/dev/null | /usr/bin/head -n 1 || true)
          ;;
      esac
    fi
    sanitize "$label"
    label_safe=$sanitized
    sanitize "$target"
    target_safe=$sanitized
    sanitize "$program_name"
    program_safe=$sanitized
    sanitize "$artefact_version"
    artefact_version_safe=$sanitized
    sanitize "$installed_version"
    installed_version_safe=$sanitized
    printf '%s{"label":"%s","current_target":"%s","program":"%s","artefact_epoch":"%s","installed_epoch":"%s","artefact_version":"%s","installed_version":"%s"}' \
      "$separator" "$label_safe" "$target_safe" "$program_safe" "$artefact_epoch" "$installed_epoch" \
      "$artefact_version_safe" "$installed_version_safe"
    separator=,
  done
fi

