# The sanitizer is checked before anything is reported, never trusted. Every
# string below depends on it and on nothing else, so a host where it does not
# do its job says so in a field of its own instead of returning a report full
# of empty strings that reads like a host with no names on it.
# The state is emitted at the end of the payload rather than the start,
# because a fault found while sanitizing the report's own fields has to be
# able to reach the field that reports it.
sanitizer_state=ok
sanitize 'probe-Value_1.2'
if [ "$sanitized" != 'probe-Value_1.2' ]; then
  sanitizer_state=broken
fi
sanitize 'a"b'
if [ "$sanitized" != 'a?b' ]; then
  sanitizer_state=broken
fi
sanitize_long=0123456789
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize "$sanitize_long"
if [ "${#sanitized}" -ne "$field_limit" ]; then
  sanitizer_state=broken
fi

if [ -L "$forward_dir" ]; then
  forwards_dir_state=symlink
elif [ -d "$forward_dir" ]; then
  forwards_dir_state=directory
elif [ -e "$forward_dir" ]; then
  forwards_dir_state=not_directory
else
  forwards_dir_state=missing
fi

printf '{"release_platform":"%s","forwards_dir_state":"%s","managed_binaries":[' \
  "$release_platform" "$forwards_dir_state"

separator=""
while IFS='	' read -r binary_name binary_root version_argument version_shape; do
  [ -n "$binary_name" ] || continue
  binary_path="$HOME/$binary_root/$binary_name"
  state=missing
  regular=false
  executable=false
  version_state=missing
  version=""
  # -L first, and never -L then -f: -f follows the link, so testing it first
  # would report a symlink to /etc/passwd as a present regular binary.
  if [ -L "$binary_path" ]; then
    state=symlink
    version_state=refused_symlink
  elif [ -f "$binary_path" ]; then
    state=present
    regular=true
    if [ -x "$binary_path" ]; then
      executable=true
      # `stado --version` answers in one plain line; `skarbiec version`
      # answers with a JSON object whose `version` member is the build. Which
      # question to ask, and which shape the answer has, are declared per
      # product rather than decided by this program: taking line one
      # unconditionally reported `{` for skarbiec, which the sanitizer then
      # correctly reduced to `?`, and a brace is not a version.
      if version_output=$("$binary_path" "$version_argument" 2>/dev/null); then
        version_rc=0
      else
        version_rc=1
      fi
      if [ "$version_rc" -ne 0 ]; then
        # Present, executable, and it did not answer. On a host out of
        # process slots that is what a failed fork looks like from here, and
        # it is reported as a state rather than as a blank version.
        version_state=version_failed
      elif [ -z "$version_output" ]; then
        version_state=version_empty
      else
        case "$version_shape" in
          json)
            case "$version_output" in
              *'"version"'*)
                version_rest=${version_output#*'"version"'}
                version_gap=${version_rest%%'"'*}
                case "$version_rest" in
                  *'"'*)
                    # Only whitespace and the colon may sit between the key
                    # and the opening quote of its value. Anything else means
                    # the next quote belongs to a different member — a null
                    # version, for one — and there is no version to report.
                    case "$version_gap" in
                      *[!:[:space:]]*) ;;
                      *)
                        version_rest=${version_rest#*'"'}
                        version=${version_rest%%'"'*}
                        ;;
                    esac
                    ;;
                esac
                ;;
            esac
            ;;
          *)
            # First line only, without the `| head -n 1` this used to fork
            # for: a plain answer may be followed by build details.
            version=${version_output%%"$newline"*}
            ;;
        esac
        if [ -n "$version" ]; then
          version_state=reported
        else
          # It answered in a shape no version could be read out of. Saying so
          # beats reporting a brace, or a fragment of some other member, as
          # this host's build.
          version_state=version_unparsable
        fi
      fi
    else
      version_state=not_executable
    fi
  elif [ -e "$binary_path" ]; then
    state=not_regular
    version_state=refused_not_regular
  fi
  sanitize "$version"
  printf '%s{"name":"%s","state":"%s","regular_file":%s,"executable":%s,"version_state":"%s","version":"%s"}' \
    "$separator" "$binary_name" "$state" "$regular" "$executable" "$version_state" "$sanitized"
  separator=,
done <<EOF
$managed_programs
EOF

