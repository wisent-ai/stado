#!/bin/sh
set -eu

runner_version=2.336.0
runner_sha256=8e8839c49b7060b6b2154f4931f815df330c27f167d53ef2239ee3dfce28b079
runner_name=stado-publisher-charless-mac-mini
runner_root="$HOME/.stado/actions-runner-wisent-backend-publisher"
token_file="$HOME/.stado/github-wisent-backend-publisher-token"
launcher="$runner_root/start-runner.sh"
plist="$HOME/Library/LaunchAgents/com.wisent.actions-runner.wisent-backend-publisher.plist"
archive=$(mktemp)
cleanup() {
  rm -f "$archive" "$token_file"
}
trap cleanup EXIT HUP INT TERM

if [ ! -f "$token_file" ]; then
  printf '%s\n' "missing owner-only registration token: $token_file" >&2
  exit 1
fi
chmod 600 "$token_file"

uid=$(id -u)
launchctl bootout "gui/$uid/com.wisent.actions-runner.wisent-backend-publisher" >/dev/null 2>&1 || true
rm -rf "$runner_root"
mkdir -p "$runner_root" "$HOME/Library/LaunchAgents"

curl --fail --silent --show-error --location --max-time 120 \
  "https://github.com/actions/runner/releases/download/v$runner_version/actions-runner-osx-arm64-$runner_version.tar.gz" \
  -o "$archive"
actual=$(shasum -a 256 "$archive" | cut -d' ' -f1)
if [ "$actual" != "$runner_sha256" ]; then
  printf '%s\n' "runner checksum mismatch: $actual" >&2
  exit 1
fi
tar -xzf "$archive" -C "$runner_root"
codesign --remove-signature "$runner_root/bin/Runner.Listener"
codesign --remove-signature "$runner_root/bin/Runner.Worker"
mkdir -p "$runner_root/_work" "$runner_root/_diag"

TOKEN_FILE="$token_file" /bin/bash -c '
  cd "$1"
  read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"
  export ACTIONS_RUNNER_INPUT_TOKEN
  export ACTIONS_RUNNER_INPUT_URL=https://github.com/wisent-ai/wisent-backend
  export ACTIONS_RUNNER_INPUT_NAME="$2"
  export ACTIONS_RUNNER_INPUT_LABELS=stado-publisher
  export ACTIONS_RUNNER_INPUT_WORK=_work
  exec ./config.sh --unattended --replace --disableupdate
' bash "$runner_root" "$runner_name"
rm -f "$token_file"

cat > "$launcher" <<'LAUNCHER'
#!/bin/sh
set -eu
export HOME=/Users/charles
export PATH="$HOME/.stado/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export ACTIONS_RUNNER_HOOK_JOB_STARTED="$HOME/.stado/actions-runner-wisent-backend-publisher/clean-work.sh"
export ACTIONS_RUNNER_HOOK_JOB_COMPLETED="$HOME/.stado/actions-runner-wisent-backend-publisher/clean-work.sh"
exec "$HOME/.stado/actions-runner-wisent-backend-publisher/bin/runsvc.sh"
LAUNCHER
chmod 755 "$launcher"

cat > "$runner_root/clean-work.sh" <<'CLEAN'
#!/bin/sh
set -eu
find "$HOME/.stado/actions-runner-wisent-backend-publisher/_work" \
  -mindepth 1 -maxdepth 1 ! -name '_*' -exec rm -rf -- {} +
CLEAN
chmod 755 "$runner_root/clean-work.sh"

cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.wisent.actions-runner.wisent-backend-publisher</string>
<key>ProgramArguments</key><array><string>$launcher</string></array>
<key>WorkingDirectory</key><string>$runner_root</string>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>5</integer>
<key>StandardOutPath</key><string>$runner_root/_diag/launchd.stdout.log</string>
<key>StandardErrorPath</key><string>$runner_root/_diag/launchd.stderr.log</string>
</dict></plist>
PLIST
plutil -lint "$plist" >/dev/null
launchctl bootstrap "gui/$uid" "$plist"
launchctl enable "gui/$uid/com.wisent.actions-runner.wisent-backend-publisher"
launchctl kickstart -k "gui/$uid/com.wisent.actions-runner.wisent-backend-publisher"
launchctl print "gui/$uid/com.wisent.actions-runner.wisent-backend-publisher" | grep -F 'state = running' >/dev/null

printf '%s\n' "wisent-backend publisher runner registered and running as $runner_name"
