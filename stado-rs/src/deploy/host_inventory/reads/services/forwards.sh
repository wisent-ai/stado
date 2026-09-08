printf ',"forwards":['
separator=""
if [ "$forwards_dir_state" = directory ]; then
  for marker_path in "$forward_dir"/*.url; do
    # An unmatched glob stays literal; a dangling symlink fails -e but not -L.
    if [ ! -e "$marker_path" ] && [ ! -L "$marker_path" ]; then
      continue
    fi
    # basename(1) by parameter expansion. Two fewer forks per marker, and the
    # marker name is one of the fields that came back empty when they failed.
    marker_name=${marker_path##*/}
    marker_name=${marker_name%.url}
    url=""
    if [ -L "$marker_path" ]; then
      marker_state=refused_symlink
    elif [ ! -f "$marker_path" ]; then
      marker_state=refused_not_regular
    elif [ ! -r "$marker_path" ]; then
      marker_state=refused_unreadable
    else
      marker_state=read
      # The read builtin, not `head -c 4096 | head -n 1`: one line, no fork,
      # and no pipeline that can die and hand back an empty string. A marker
      # is one short loopback URL, and -n bounds the read so a
      # multi-gigabyte file is never pulled in.
      IFS= read -r -n 4096 url < "$marker_path" || true
      if [ -z "$url" ]; then
        # The file is there and its first line is empty. That is a state,
        # not a URL the other side should be left to guess at.
        marker_state=read_empty
      fi
    fi
    sanitize "$marker_name"
    marker_name_safe=$sanitized
    sanitize "$url"
    printf '%s{"name":"%s","state":"%s","url":"%s"}' \
      "$separator" "$marker_name_safe" "$marker_state" "$sanitized"
    separator=,
  done
fi

