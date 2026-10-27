//! Fixed, registry-authorized host programs; user commands run only inside gVisor.
use crate::deploy::shlex_quote;
use std::num::{NonZeroU32, NonZeroU64};

pub const PRELUDE: &str = r#"
fail() { printf '%s\n' "$*" >&2; exit 1; }
command -v docker >/dev/null || fail 'Docker CLI is missing; install the sandbox runtime on this host'
docker info >/dev/null
lock() {
  command -v flock >/dev/null || fail 'flock is missing; install util-linux before sandbox operations'
  exec {lock_fd}>"/run/lock/stado-fleet-sandboxes.lock"
  flock "$lock_fd"
}
managed() {
  owner=$(docker inspect --format '{{index .Config.Labels "com.wisent.stado.sandbox"}}' "$container")
  [ "$owner" = fleet ] || fail "container $container is not a managed fleet sandbox; refusing access"
}
receipt() { printf 'STADO_SANDBOX\t%s\n' "$1"; }
cgroup_controls() {
  pid=$(docker inspect --format '{{.State.Pid}}' "$container")
  cgroup=
  while IFS=: read -r hierarchy controllers path; do
    if [ "$hierarchy" = 0 ] && [ -z "$controllers" ]; then cgroup=$path; fi
  done <"/proc/$pid/cgroup"
  [ -n "$cgroup" ] || fail "no cgroup v2 membership was observed for sandbox process $pid"
  controls=/sys/fs/cgroup$cgroup
}
kernel_limits() {
  running=$(docker inspect --format '{{.State.Running}}' "$container")
  if [ "$running" != true ]; then return; fi
  cgroup_controls
  read -r quota period <"$controls/cpu.max"
  read -r memory <"$controls/memory.max"
  read -r swap <"$controls/memory.swap.max"
  read -r pids <"$controls/pids.max"
  for value in "$quota" "$period" "$memory" "$swap" "$pids"; do
    [[ "$value" =~ ^(max|[0-9]+)$ ]] || fail "unreadable kernel resource control in $controls"
  done
  printf 'STADO_SANDBOX_LIMITS\t{"cpu_quota":"%s","cpu_period":"%s","memory_bytes":"%s","swap_bytes":"%s","pids_limit":"%s"}\n' "$quota" "$period" "$memory" "$swap" "$pids"
}
observe() {
  managed
  result=$(docker inspect --format '{{json .}}' "$container")
  kernel_limits
  receipt "$result"
}
"#;

pub fn inventory() -> &'static str {
    r#"
runtimes=$(docker info --format '{{json .Runtimes}}')
cgroup=$(docker info --format '{{.CgroupVersion}}')
printf 'Docker runtimes: %s\nCgroup version: %s\n' "$runtimes" "$cgroup"
case "$runtimes" in *'"runsc"'*) ;; *) fail 'gVisor runsc is not registered in Docker; no runc fallback is allowed' ;; esac
[ "$cgroup" = 2 ] || fail "cgroup v2 is required to enforce limits; observed $cgroup"
receipt "$(docker info --format '{{json .}}')"
"#
}
pub fn list() -> &'static str {
    r#"
docker ps --all --filter label=com.wisent.stado.sandbox=fleet --format '{{json .}}'
receipt '{"listed":true}'
"#
}
pub fn show() -> &'static str {
    "observe"
}

/// The exec program. Each passed variable ([`super::passed`]) is exported in
/// the host shell and handed to `docker exec -e NAME` by name, so its value
/// is in no argv and is printed nowhere.
pub fn execute(command: &str, detached: bool, passed: &[(String, String)]) -> String {
    let exports: String = passed
        .iter()
        .map(|(name, value)| format!("export {name}={}\n", shlex_quote(value)))
        .collect();
    let flags: String = passed
        .iter()
        .map(|(name, _)| format!(" -e {name}"))
        .collect();
    if !detached {
        return format!(
            r#"
managed
{exports}set +e
docker exec{flags} "$container" /bin/sh -c {command}
code=$?
set -e
receipt "{{\"completed\":true,\"exit_code\":$code}}"
exit "$code"
"#,
            command = shlex_quote(command)
        );
    }
    let id = uuid::Uuid::new_v4().to_string();
    let directory = format!("/workspace/.stado-executions/{id}");
    let program = format!(
        r#"umask 077
mkdir -p {directory}
printf '%s\n' running >{directory}/state
/bin/sh -c {command} >{directory}/stdout 2>{directory}/stderr
code=$?
printf '%s\n' "$code" >{directory}/exit-code
printf '%s\n' completed >{directory}/state
exit "$code"
"#,
        command = shlex_quote(command)
    );
    format!(r#"
managed
{exports}docker exec "$container" /bin/sh -c {prepare}
docker exec --detach{flags} "$container" /bin/sh -c {program}
receipt '{{"execution":"{id}","accepted":true,"directory":"{directory}"}}'
"#, program=shlex_quote(&program), prepare=shlex_quote(&format!("umask 077; mkdir -p {directory}; : >{directory}/stdout; : >{directory}/stderr; printf '%s\\n' accepted >{directory}/state")))
}

pub fn logs(execution: &str) -> String {
    let directory = format!("/workspace/.stado-executions/{execution}");
    let program = format!(
        r#"test -f {directory}/state || {{ echo 'execution is absent or has not started' >&2; exit 1; }}
printf 'STATE\t'; cat {directory}/state
printf 'STDOUT\n'; cat {directory}/stdout
printf 'STDERR\n'; cat {directory}/stderr
if test -f {directory}/exit-code; then printf 'EXIT_CODE\t'; cat {directory}/exit-code; fi
"#
    );
    format!(
        r#"
managed
docker exec "$container" /bin/sh -c {program}
receipt '{{"execution":"{execution}","read":true}}'
"#,
        program = shlex_quote(&program)
    )
}

pub fn update(cpus: NonZeroU32, memory: NonZeroU64, pids: NonZeroU32) -> String {
    format!(
        r#"
lock
managed
set +e
docker update --cpus {cpus} --memory {memory} --memory-swap {memory} --pids-limit {pids} "$container"
code=$?
set -e
if [ "$code" != 0 ]; then
  observe
  exit "$code"
fi
[ "$(docker inspect --format '{{{{.HostConfig.NanoCpus}}}}' "$container")" = {cpus}000000000 ] || fail 'CPU limit verification failed'
[ "$(docker inspect --format '{{{{.HostConfig.Memory}}}}' "$container")" = {memory} ] || fail 'memory limit verification failed'
[ "$(docker inspect --format '{{{{.HostConfig.MemorySwap}}}}' "$container")" = {memory} ] || fail 'swap limit verification failed'
[ "$(docker inspect --format '{{{{.HostConfig.PidsLimit}}}}' "$container")" = {pids} ] || fail 'process limit verification failed'
observe
"#
    )
}
/// Stop or resume the sandbox, and its egress proxy with it: the proxy
/// starts before the sandbox resumes and stops after the sandbox stops, so
/// the sandbox never runs with its declared egress missing.
pub fn transition(resume: bool) -> String {
    let (verb, state) = if resume {
        ("start", "running")
    } else {
        ("stop", "exited")
    };
    let (before, after) = match resume {
        true => ("proxy_transition", ""),
        false => ("", "proxy_transition"),
    };
    format!(
        r#"
{egress}
lock
managed
egress=
egress_names
proxy_transition() {{
  if [ -n "$(docker ps --all --filter "name=^/$proxy$" --format '{{{{.Names}}}}')" ]; then
    docker {verb} "$proxy"
    proxy_state=$(docker inspect --format '{{{{.State.Status}}}}' "$proxy")
    [ "$proxy_state" = {state} ] || fail "egress proxy $proxy is $proxy_state after {verb}, expected {state}"
  fi
}}
{before}
docker {verb} "$container"
state=$(docker inspect --format '{{{{.State.Status}}}}' "$container")
[ "$state" = {state} ] || fail "sandbox $name is $state after {verb}, expected {state}"
{after}
observe
"#,
        egress = super::egress::EGRESS,
    )
}
/// Delete the sandbox, its workspace and its egress proxy, network and
/// proxy configuration, confirming each is absent.
pub fn delete() -> String {
    format!(
        r#"
{egress}
lock
egress=
egress_names
found=$(docker ps --all --filter "name=^/$container$" --format '{{{{.Names}}}}')
workspace=$(docker volume ls --filter "name=^$volume$" --format '{{{{.Name}}}}')
if [ -n "$found" ]; then managed; fi
if [ -n "$workspace" ]; then
  owner=$(docker volume inspect --format '{{{{index .Labels "com.wisent.stado.sandbox"}}}}' "$volume")
  [ "$owner" = fleet ] || fail "workspace $volume is not managed; nothing was deleted"
fi
if [ -n "$found" ]; then docker container rm --force "$container"; fi
remove_egress
if [ -n "$workspace" ]; then docker volume rm "$volume"; fi
remaining=$(docker ps --all --filter "name=^/$container$" --format '{{{{.Names}}}}')
[ -z "$remaining" ] || fail 'sandbox still exists after deletion'
remaining=$(docker volume ls --filter "name=^$volume$" --format '{{{{.Name}}}}')
[ -z "$remaining" ] || fail 'workspace still exists after deletion'
receipt '{{"container":"absent","workspace":"absent"}}'
"#,
        egress = super::egress::EGRESS,
    )
}
/// The usage program: what the running sandbox has used since its last
/// start, read from its own cgroup v2 counters (`cpu.stat` usage_usec,
/// `memory.peak`, `memory.current`, `pids.current`) and its workspace size,
/// with the start time those counters begin at. A stopped sandbox has no
/// counters and is refused; a kernel without `memory.peak` is named.
pub fn usage() -> &'static str {
    r#"
managed
[ "$(docker inspect --format '{{.State.Running}}' "$container")" = true ] || fail "sandbox $name is not running; its usage counters exist only while it runs since its last start"
cgroup_controls
usage=
while read -r key value; do if [ "$key" = usage_usec ]; then usage=$value; fi; done <"$controls/cpu.stat"
[ -f "$controls/memory.peak" ] || fail "this kernel exposes no memory.peak in $controls; peak memory cannot be measured"
read -r peak <"$controls/memory.peak"
read -r current <"$controls/memory.current"
read -r processes <"$controls/pids.current"
for value in "$usage" "$peak" "$current" "$processes"; do
  [[ "$value" =~ ^[0-9]+$ ]] || fail "unreadable usage counter in $controls"
done
workspace=$(docker exec "$container" du -sk /workspace)
workspace=${workspace%%[[:space:]]*}
[[ "$workspace" =~ ^[0-9]+$ ]] || fail "the workspace size could not be read: $workspace"
started=$(docker inspect --format '{{.State.StartedAt}}' "$container")
receipt "{\"since\":\"$started\",\"cpu_usage_usec\":$usage,\"memory_peak_bytes\":$peak,\"memory_current_bytes\":$current,\"processes\":$processes,\"workspace_kib\":$workspace}"
"#
}
