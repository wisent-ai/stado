
# A path's modification time in seconds, asked of the host's own stat and
# left in $mtime_read. The inventory's two artefact epochs and the reclaim
# program's second look splice this one function, so those reads choose the
# dialect in one place; the program's other modification-time reads keep
# their own per-kernel spelling. GNU stat reads -f as "the file system", so
# `stat -f %m PATH` on Linux answers several lines about the file system
# holding PATH instead of a time. The function reads the kernel's name for
# itself, as $mtime_kernel, when it is spliced in.
# Anything but one integer, signed for a time before 1970, answers 1 with
# stat's own answer in $mtime_read, for the caller to report.
mtime_kernel=$(/usr/bin/uname -s 2>/dev/null || /bin/uname -s)
mtime_seconds() {
  if [ "$mtime_kernel" = Darwin ]; then
    mtime_read=$(/usr/bin/stat -f %m -- "$1" 2>&1) || return 1
  else
    mtime_read=$(/usr/bin/stat -c %Y -- "$1" 2>&1) || return 1
  fi
  case "${mtime_read#-}" in
    '' | *[!0-9]*)
      mtime_read="stat answered no time: $mtime_read"
      return 1
      ;;
  esac
}
