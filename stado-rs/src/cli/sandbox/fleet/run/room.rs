//! What each fleet host can give a sandbox now, read from the host itself:
//! its sandbox prerequisites, its cores, RAM and NVIDIA GPUs, what its
//! running fleet sandboxes already hold, and every fleet sandbox it keeps.
//!
//! The reading is one read-only program per host, sent to every Linux host
//! of the registry at once. Nothing is taken from a store another product
//! keeps: a host answers for itself, and a host that does not answer is
//! named with its own failure instead of being counted as empty.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::targets::ComputeTarget;

/// The read-only program: prerequisites as facts rather than refusals, so a
/// host lacking gVisor still reports the sandboxes it holds. Held resources
/// are the enforced limits of running fleet sandboxes (`HostConfig`), not
/// their labels, so an `update` is counted as it stands; a stopped sandbox
/// holds nothing until it is resumed. A GPU request of `-1` is Docker's
/// "every GPU" and holds them all.
const PROGRAM: &str = r#"
runtimes=$(docker info --format '{{json .Runtimes}}')
runsc=false
case "$runtimes" in *'"runsc"'*) runsc=true ;; esac
nvproxy=false
case "$(docker info --format '{{json .Runtimes.runsc}}')" in *'--nvproxy'*) nvproxy=true ;; esac
cgroup=$(docker info --format '{{.CgroupVersion}}')
cores=$(nproc)
memory_available=$(( $(awk '/^MemAvailable:/ {print $2}' /proc/meminfo) * 1024 ))
memory_total=$(( $(awk '/^MemTotal:/ {print $2}' /proc/meminfo) * 1024 ))
gpus=0
if command -v nvidia-smi >/dev/null; then
  gpus=$(nvidia-smi --list-gpus | grep -c '^GPU ' || true)
fi
docker ps --all --filter label=com.wisent.stado.sandbox=fleet --format '{{json .}}' | while IFS= read -r line; do
  [ -z "$line" ] || printf 'STADO_SANDBOX_HELD\t%s\n' "$line"
done
held_nano_cpus=0
held_memory=0
held_gpus=0
for id in $(docker ps --quiet --filter label=com.wisent.stado.sandbox=fleet); do
  read -r nano memory requested < <(docker inspect --format '{{.HostConfig.NanoCpus}} {{.HostConfig.Memory}} {{range .HostConfig.DeviceRequests}}{{.Count}}{{end}}' "$id")
  held_nano_cpus=$(( held_nano_cpus + nano ))
  held_memory=$(( held_memory + memory ))
  if [ "${requested:-0}" = -1 ]; then held_gpus=$gpus; else held_gpus=$(( held_gpus + ${requested:-0} )); fi
done
for value in "$cores" "$memory_available" "$memory_total" "$gpus" "$held_nano_cpus" "$held_memory" "$held_gpus"; do
  [[ "$value" =~ ^[0-9]+$ ]] || fail "unreadable host resource reading: $value"
done
printf 'STADO_SANDBOX_ROOM\t{"runsc":%s,"nvproxy":%s,"cgroup":"%s","cores":%s,"memory_available_bytes":%s,"memory_total_bytes":%s,"gpus":%s,"held_nano_cpus":%s,"held_memory_bytes":%s,"held_gpus":%s}\n' \
  "$runsc" "$nvproxy" "$cgroup" "$cores" "$memory_available" "$memory_total" "$gpus" "$held_nano_cpus" "$held_memory" "$held_gpus"
"#;

/// One host's answer to [`PROGRAM`].
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub(super) struct Room {
    pub runsc: bool,
    pub nvproxy: bool,
    pub cgroup: String,
    pub cores: u64,
    pub memory_available_bytes: u64,
    pub memory_total_bytes: u64,
    pub gpus: u64,
    pub held_nano_cpus: u64,
    pub held_memory_bytes: u64,
    pub held_gpus: u64,
}

impl Room {
    /// Cores not yet promised to a running fleet sandbox, in Docker's
    /// nano-CPUs.
    pub fn free_nano_cpus(&self) -> u64 {
        self.cores
            .saturating_mul(1_000_000_000)
            .saturating_sub(self.held_nano_cpus)
    }

    /// RAM a new sandbox can be promised: what the kernel says is available
    /// now, and no more than what running fleet sandboxes have not already
    /// been promised, whichever is less.
    pub fn free_memory_bytes(&self) -> u64 {
        self.memory_available_bytes.min(
            self.memory_total_bytes
                .saturating_sub(self.held_memory_bytes),
        )
    }

    pub fn free_gpus(&self) -> u64 {
        self.gpus.saturating_sub(self.held_gpus)
    }
}

/// What one host answered: its room and the fleet sandboxes it keeps
/// (Docker's listing lines), or why it gave neither.
pub(super) struct Reading {
    pub host: String,
    pub answer: Result<(Room, Vec<Value>), String>,
}

impl Reading {
    /// Whether this host keeps the fleet sandbox `name`.
    pub fn holds(&self, name: &str) -> bool {
        let container = format!("stado-sandbox-{name}");
        self.answer.as_ref().is_ok_and(|(_, held)| {
            held.iter()
                .any(|entry| entry.get("Names").and_then(Value::as_str) == Some(&container))
        })
    }
}

/// Every registry target a fleet sandbox could be on: a local host on
/// Linux reachable through its channel or being this machine. The others
/// are returned with why they are not, so a refusal names every host.
pub(super) async fn candidates() -> Result<(Vec<ComputeTarget>, Vec<Value>), CmdError> {
    let registry = host_channel::canonical_registry().await?;
    let mut usable = Vec::new();
    let mut passed = Vec::new();
    for target in registry.targets {
        let reason = if !target.is_provider(crate::capabilities::ProviderId::Local) {
            Some(format!(
                "{} is a {} target, not a registered host",
                target.name, target.kind
            ))
        } else if !target.release_platform.starts_with("linux-") {
            Some(format!(
                "{} runs {}; fleet sandboxes need Linux with Docker, cgroup v2 and gVisor runsc",
                target.name, target.release_platform
            ))
        } else if !target.has_ssh_connection() && !host_channel::target_is_this_host(&target) {
            Some(format!(
                "{} has no registry-managed ssh destination and is not this host",
                target.name
            ))
        } else {
            None
        };
        match reason {
            Some(reason) => {
                passed.push(json!({"host": target.name, "considered": false, "reason": reason}))
            }
            None => usable.push(target),
        }
    }
    Ok((usable, passed))
}

/// Read every candidate at once. Each read is the host channel's own,
/// announced in the register of open waits like any other host program.
pub(super) async fn read(targets: &[ComputeTarget]) -> Vec<Reading> {
    let runner = crate::deploy::production_runner();
    let script = format!(
        "set -euo pipefail\n{}\n{PROGRAM}",
        super::super::programs::PRELUDE
    );
    let reads = targets.iter().map(|target| {
        let (runner, script) = (&runner, &script);
        async move {
            let answer = match host_channel::run_script(target, script, runner).await {
                Ok(output) if output.succeeded() => parse(&output.stdout),
                Ok(output) => Err(format!(
                    "its reading exited {}: {}",
                    output.code,
                    output.detail().trim()
                )),
                Err(error) => Err(error.to_string()),
            };
            Reading {
                host: target.name.clone(),
                answer,
            }
        }
    });
    futures::future::join_all(reads).await
}

fn parse(stdout: &str) -> Result<(Room, Vec<Value>), String> {
    let room = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("STADO_SANDBOX_ROOM\t"))
        .ok_or("its reading printed no resource line")?;
    let room: Room = serde_json::from_str(room)
        .map_err(|error| format!("its resource line is unreadable: {error}"))?;
    let held = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("STADO_SANDBOX_HELD\t"))
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("its sandbox listing is unreadable: {error}"))?;
    Ok((room, held))
}
