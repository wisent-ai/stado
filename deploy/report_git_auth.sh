#!/usr/bin/env bash
# Report how this host authenticates to private git remotes, without printing one.
#
# brama's release build fetches two private dependencies. The mac builder resolves
# them; the Linux builder fails with `failed to acquire username/password from
# local configuration`, so the quality gate passes and the build dies fetching.
# Whatever the mac has, the Linux host needs the equivalent -- and the difference
# has to be read rather than assumed.
#
# Read-only: mechanism names, file presence and sizes only. No token, no key, and
# no remote is contacted.
set -euo pipefail

printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'user %s\n' "$(id -un)"
printf 'git %s\n' "$(command -v git || echo MISSING)"
command -v git >/dev/null || exit 0

printf 'credential_helper %s\n' "$(git config --get-all credential.helper 2>/dev/null | tr '\n' ',' || echo none)"
printf 'insteadof_rules %s\n' "$(git config --get-regexp '^url\..*\.insteadof$' 2>/dev/null | wc -l | tr -d ' ')"
printf 'askpass %s\n' "${GIT_ASKPASS:-unset}"
printf 'gh_cli %s\n' "$(command -v gh >/dev/null && echo present || echo MISSING)"

for candidate in "$HOME/.git-credentials" "$HOME/.config/gh/hosts.yml" "$HOME/.netrc"; do
  if [ -f "$candidate" ]; then
    printf 'store %s bytes=%s mode=%s\n' "$candidate" \
      "$(/usr/bin/wc -c <"$candidate" | /usr/bin/tr -d ' ')" \
      "$(/usr/bin/stat -f '%Sp' "$candidate" 2>/dev/null || /usr/bin/stat -c '%A' "$candidate")"
  else
    printf 'store %s absent\n' "$candidate"
  fi
done

# An SSH key plus an insteadOf rule is the other way this is usually solved.
for key in "$HOME/.ssh/id_ed25519" "$HOME/.ssh/id_rsa"; do
  [ -f "$key" ] && printf 'ssh_key %s present\n' "$key" || printf 'ssh_key %s absent\n' "$key"
done

printf 'cargo_git_cli %s\n' "$(git config --get net.git-fetch-with-cli 2>/dev/null || echo unset)"
if [ -f "$HOME/.cargo/config.toml" ]; then
  printf 'cargo_config git-fetch-with-cli=%s\n' \
    "$(/usr/bin/grep -c 'git-fetch-with-cli' "$HOME/.cargo/config.toml" || true)"
else
  printf 'cargo_config absent\n'
fi
