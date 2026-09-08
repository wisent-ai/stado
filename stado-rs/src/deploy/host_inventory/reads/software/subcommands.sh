stado_probe=no
if [ ! -L "$bin_dir/stado" ] && [ -f "$bin_dir/stado" ] && [ -x "$bin_dir/stado" ]; then
  stado_probe=yes
fi
separator=""
# Ask for the subcommand's HELP, never run the subcommand: clap exits zero for
# a path it knows and non-zero for one it does not, so the exit code answers
# the version-skew question without the host performing the action.
probe_subcommand() {
  subcommand_name="$*"
  if [ "$stado_probe" = yes ]; then
    if "$bin_dir/stado" "$@" --help >/dev/null 2>&1; then
      subcommand_state=present
    else
      subcommand_rc=$?
      if [ "$subcommand_rc" -ge 126 ]; then
        # 126 and 127 are "could not execute", and a shell that cannot fork
        # reports in the same band. The binary never got to answer, so the
        # answer is not "this subcommand is absent".
        subcommand_state=probe_failed
      else
        subcommand_state=absent
      fi
    fi
  else
    subcommand_state=unavailable
  fi
  sanitize "$subcommand_name"
  printf '%s{"name":"%s","state":"%s"}' \
    "$separator" "$sanitized" "$subcommand_state"
  separator=,
}
probe_subcommand host inventory
probe_subcommand route list
probe_subcommand host exec
probe_subcommand service list
probe_subcommand registry doctor

