#!/bin/sh
# Every symlink on this host that still points into a removed disk.
#
# When the 16 TB volume left the machine, each symlink into `/mnt/wd16tb`
# became a path that exists and resolves to nothing. `mkdir` answers EEXIST for
# exactly that, so every consumer failed differently and none of them named the
# link: the agent said `agent loop failed: File exists (os error 17)` and
# rustup said `could not create home directory: '/root/.rustup': File exists`.
# One cause, one report.
#
# Read-only.
set -eu

printf 'MOUNT_STATE\n'
if awk '$2 == "/mnt/wd16tb"' /proc/self/mounts | grep -q .; then
  printf '/mnt/wd16tb\tmounted\n'
else
  printf '/mnt/wd16tb\tnot mounted (directory on %s)\n' \
    "$(df -P /mnt/wd16tb 2>/dev/null | awk 'NR==2 { print $1 }')"
fi

printf '\nDANGLING_LINKS\n'
found=0
for root in /root /home /opt /srv /var/lib /etc/systemd /usr/local; do
  [ -d "$root" ] || continue
  # -xdev keeps this off the docker overlay tree, which holds renters' images
  # and answers nothing about fleet configuration.
  find "$root" -xdev -type l 2>/dev/null | while IFS= read -r link; do
    target=$(readlink "$link")
    case "$target" in
      /mnt/wd16tb*|*wd16tb*) ;;
      *) continue ;;
    esac
    if [ -e "$link" ]; then
      printf '%s\t-> %s\tresolves\n' "$link" "$target"
    else
      printf '%s\t-> %s\tDANGLING\n' "$link" "$target"
    fi
  done
  found=1
done
[ "$found" -eq 1 ] || printf 'no roots scanned\n'

printf '\nUNIT_REFERENCES\n'
grep -rl 'wd16tb' /etc/systemd/system /root/.config/systemd 2>/dev/null || printf 'none\n'

printf '\nCONFIG_REFERENCES\n'
grep -rl 'wd16tb' /root/.config /root/.stado/*.env 2>/dev/null || printf 'none\n'
