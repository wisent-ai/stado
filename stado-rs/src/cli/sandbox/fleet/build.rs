//! The one way a fleet sandbox is built on its host, shared by `create` and
//! `fork`, and the `fork` program: a new sandbox with the same image, the
//! source's enforced limits, lifetime and egress, and a byte-verified copy
//! of its workspace.

use std::num::{NonZeroU32, NonZeroU64};

use clap::Args;

use crate::cli::CmdError;
use crate::deploy::shlex_quote;

/// What `create` builds, as the caller declares it.
#[derive(Args)]
pub struct Spec {
    /// Immutable IMAGE@sha256:DIGEST, with /bin/sh and sleep.
    #[arg(long)]
    image: String,
    #[arg(long)]
    cpus: NonZeroU32,
    /// RAM ceiling in bytes; swap is disabled.
    #[arg(long)]
    memory_bytes: NonZeroU64,
    #[arg(long)]
    pids_limit: NonZeroU32,
    /// Stop the sandbox this many seconds after each start, keeping its
    /// workspace; resume starts the count again. Without it the sandbox
    /// runs until stop or delete: Stado declares no lifetime of its own.
    #[arg(long)]
    stop_after_seconds: Option<NonZeroU64>,
    /// A host name the sandbox may reach over HTTP or HTTPS through its own
    /// egress proxy; repeatable. Without one the sandbox has no network.
    #[arg(long = "egress", value_name = "HOST")]
    egress: Vec<String>,
    /// Immutable IMAGE@sha256:DIGEST holding tinyproxy, the sandbox's own
    /// egress proxy; required with --egress, refused without it.
    #[arg(long)]
    egress_proxy_image: Option<String>,
    /// How many of the host's NVIDIA GPUs the sandbox gets, through gVisor's
    /// nvproxy (runsc registered with --nvproxy); without it, none.
    #[arg(long)]
    gpus: Option<NonZeroU32>,
}

/// Whether `image` names an immutable image by its digest.
fn immutable(image: &str) -> bool {
    regex::Regex::new(r"^[^\s@-][^\s@]*@sha256:[a-f0-9]{64}$")
        .expect("OCI digest grammar")
        .is_match(image)
}

impl Spec {
    /// What a host must have free to take this sandbox when Stado chooses
    /// the host: its declared cores, RAM and GPUs.
    pub(super) fn demand(&self) -> super::run::Demand {
        super::run::Demand {
            cpus: self.cpus.get(),
            memory_bytes: self.memory_bytes.get(),
            gpus: self.gpus.map_or(0, NonZeroU32::get),
        }
    }
    /// The create program for this spec, or the refusal of what is not
    /// declared as it must be; nothing reaches the host before it.
    pub(super) fn program(self) -> Result<String, CmdError> {
        if !immutable(&self.image) {
            return Err(CmdError::usage(
                "--image must name an immutable IMAGE@sha256:<64 lowercase hex digits>; no sandbox was created",
            ));
        }
        let egress = super::egress::hosts(&self.egress)?;
        let proxy_image = match (egress.is_empty(), self.egress_proxy_image) {
            (true, None) => String::new(),
            (true, Some(_)) => {
                return Err(CmdError::usage(
                    "--egress-proxy-image is the proxy of declared --egress hosts, and none is declared; no sandbox was created",
                ))
            }
            (false, None) => {
                return Err(CmdError::usage(
                    "--egress needs --egress-proxy-image, the immutable image of the tinyproxy that passes only those hosts; no sandbox was created",
                ))
            }
            (false, Some(image)) if !immutable(&image) => {
                return Err(CmdError::usage(
                    "--egress-proxy-image must name an immutable IMAGE@sha256:<64 lowercase hex digits>; no sandbox was created",
                ))
            }
            (false, Some(image)) => image,
        };
        // An empty value is the empty label: no declared lifetime holds the
        // sandbox running until it is stopped, no declared GPU gives none.
        let lifetime = match self.stop_after_seconds {
            Some(seconds) => seconds.to_string(),
            None => String::new(),
        };
        let gpus = match self.gpus {
            Some(count) => count.to_string(),
            None => String::new(),
        };
        Ok(format!(
            r#"
{inventory}
{BUILD}
{egress_functions}
lock
absent
image={image}
cpus={cpus}
memory={memory}
pids={pids}
gpus={gpus}
lifetime={lifetime}
egress={egress}
proxy_image={proxy_image}
docker pull "$image"
docker volume create --label com.wisent.stado.sandbox=fleet "$volume"
trap cleanup EXIT
build_sandbox
trap - EXIT
observe
"#,
            inventory = super::programs::inventory(),
            egress_functions = super::egress::EGRESS,
            image = shlex_quote(&self.image),
            cpus = self.cpus,
            memory = self.memory_bytes,
            pids = self.pids_limit,
            egress = shlex_quote(&egress),
            proxy_image = shlex_quote(&proxy_image),
        ))
    }
}

/// A sandbox name: a lowercase letter, then lowercase letters, digits and
/// hyphens, so it names its container and volume as written.
pub(super) fn valid_name(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Shell functions the create and fork programs call once their variables
/// name the sandbox: `container`, `volume`, `image`, `cpus`, `memory`,
/// `pids`, `lifetime` (empty: no declared lifetime), and the egress
/// ([`super::egress::EGRESS`]): `egress` and `proxy_image`. The main process
/// is `sleep infinity`, or `sleep N` for a declared lifetime of N seconds, so
/// the sandbox stops by itself N seconds after each start. Every limit, the
/// lifetime label and the main process are read back after the start. A
/// failed build removes what it made, the egress proxy, its network and
/// configuration included.
pub const BUILD: &str = r#"
created_container=
created_proxy=
created_config=
created_network=
cleanup() {
  code=$?
  if [ "$code" != 0 ]; then
    if [ -n "$created_container" ]; then
      if ! docker container rm --force "$created_container"; then printf 'cleanup could not remove sandbox %s; inspect it before retrying creation\n' "$created_container" >&2; fi
    fi
    if [ -n "$created_proxy" ]; then
      if ! docker container rm --force "$created_proxy"; then printf 'cleanup could not remove egress proxy %s\n' "$created_proxy" >&2; fi
    fi
    if [ -n "$created_config" ]; then
      if ! docker volume rm "$created_config"; then printf 'cleanup could not remove proxy configuration %s\n' "$created_config" >&2; fi
    fi
    if [ -n "$created_network" ]; then
      if ! docker network rm "$created_network"; then printf 'cleanup could not remove network %s\n' "$created_network" >&2; fi
    fi
    if ! docker volume rm "$volume"; then printf 'cleanup could not remove workspace %s; sandbox fleet delete can retry its removal\n' "$volume" >&2; fi
  fi
  exit "$code"
}
absent() {
  found=$(docker ps --all --filter "name=^/$container$" --format '{{.Names}}')
  [ -z "$found" ] || fail "sandbox $name already exists; read it with sandbox fleet show"
  workspace=$(docker volume ls --filter "name=^$volume$" --format '{{.Name}}')
  [ -z "$workspace" ] || fail "workspace $volume already exists; refusing to adopt another sandbox's data"
}
build_sandbox() {
  sleep_for=${lifetime:-infinity}
  gpu_flags=()
  if [ -n "$gpus" ]; then
    runsc_args=$(docker info --format '{{json .Runtimes.runsc}}')
    case "$runsc_args" in *'--nvproxy'*) ;; *) fail "runsc is registered without --nvproxy ($runsc_args), so no GPU reaches a gVisor sandbox; no sandbox was created" ;; esac
    gpu_flags=(--gpus "$gpus")
  fi
  egress_names
  build_proxy
  created_container=$(docker create --name "$container" --label com.wisent.stado.sandbox=fleet \
    --label "com.wisent.stado.sandbox.cpus=$cpus" \
    --label "com.wisent.stado.sandbox.memory=$memory" \
    --label "com.wisent.stado.sandbox.pids=$pids" \
    --label "com.wisent.stado.sandbox.gpus=$gpus" \
    --label "com.wisent.stado.sandbox.stop-after-seconds=$lifetime" \
    --label "com.wisent.stado.sandbox.egress=$egress" \
    --label "com.wisent.stado.sandbox.egress-proxy-image=$proxy_image" \
    --runtime runsc --cpus "$cpus" --memory "$memory" --memory-swap "$memory" --pids-limit "$pids" \
    "${gpu_flags[@]}" --read-only "${network_flags[@]}" --cap-drop ALL --security-opt no-new-privileges \
    --init --mount "type=volume,source=$volume,target=/workspace" --workdir /workspace \
    --entrypoint /bin/sh "$image" -c "exec sleep $sleep_for")
  docker start "$container"
  docker exec "$container" /bin/sh -c 'test -d /workspace && test -w /workspace'
  [ "$(docker inspect --format '{{.HostConfig.Runtime}}' "$container")" = runsc ] || fail 'runtime verification failed'
  [ "$(docker inspect --format '{{.HostConfig.NanoCpus}}' "$container")" = "${cpus}000000000" ] || fail 'CPU limit verification failed'
  [ "$(docker inspect --format '{{.HostConfig.Memory}}' "$container")" = "$memory" ] || fail 'memory limit verification failed'
  [ "$(docker inspect --format '{{.HostConfig.MemorySwap}}' "$container")" = "$memory" ] || fail 'swap limit verification failed'
  [ "$(docker inspect --format '{{.HostConfig.PidsLimit}}' "$container")" = "$pids" ] || fail 'process limit verification failed'
  [ "$(docker inspect --format '{{index .Config.Labels "com.wisent.stado.sandbox.stop-after-seconds"}}' "$container")" = "$lifetime" ] || fail 'lifetime declaration verification failed'
  [ "$(docker inspect --format '{{join .Config.Cmd " "}}' "$container")" = "-c exec sleep $sleep_for" ] || fail 'lifetime enforcement verification failed'
  if [ -n "$gpus" ]; then
    [ "$(docker inspect --format '{{range .HostConfig.DeviceRequests}}{{.Count}}{{end}}' "$container")" = "$gpus" ] || fail 'GPU request verification failed'
    docker exec "$container" /bin/sh -c 'test -e /dev/nvidiactl && test -e /dev/nvidia-uvm' || fail 'no NVIDIA device reached the sandbox through nvproxy'
  fi
  [ "$(docker inspect --format '{{.State.Running}}' "$container")" = true ] || fail 'sandbox did not remain running'
}
"#;

/// The fork program, run with the header naming the source sandbox and
/// `target` the new one's name: the source must be stopped, so its
/// workspace is copied whole; the copy runs in gVisor without network or
/// capabilities, and the new sandbox is kept only when a digest of every
/// file's name and content is the same on both workspaces. The new sandbox
/// takes the source's image, currently enforced limits and lifetime.
pub fn fork(target: &str) -> String {
    format!(
        r#"
{BUILD}
{egress}
lock
managed
[ "$(docker inspect --format '{{{{.State.Running}}}}' "$container")" = false ] || fail "sandbox $name is running; stop it before forking so its workspace is copied whole"
source=$name
source_volume=$volume
image=$(docker inspect --format '{{{{.Config.Image}}}}' "$container")
cpus=$(( $(docker inspect --format '{{{{.HostConfig.NanoCpus}}}}' "$container") / 1000000000 ))
memory=$(docker inspect --format '{{{{.HostConfig.Memory}}}}' "$container")
pids=$(docker inspect --format '{{{{.HostConfig.PidsLimit}}}}' "$container")
gpus=$(docker inspect --format '{{{{index .Config.Labels "com.wisent.stado.sandbox.gpus"}}}}' "$container")
lifetime=$(docker inspect --format '{{{{index .Config.Labels "com.wisent.stado.sandbox.stop-after-seconds"}}}}' "$container")
egress=$(docker inspect --format '{{{{index .Config.Labels "com.wisent.stado.sandbox.egress"}}}}' "$container")
proxy_image=$(docker inspect --format '{{{{index .Config.Labels "com.wisent.stado.sandbox.egress-proxy-image"}}}}' "$container")
name={target}
container=stado-sandbox-$name
volume=stado-sandbox-$name
absent
docker volume create --label com.wisent.stado.sandbox=fleet "$volume"
created_container=
trap cleanup EXIT
copier() {{
  docker run --rm --runtime runsc --network none --cap-drop ALL --security-opt no-new-privileges --read-only \
    --mount "type=volume,source=$source_volume,target=/from,readonly" \
    --mount "type=volume,source=$volume,target=/to" --entrypoint /bin/sh "$image" -c "$1"
}}
copier 'cp -a /from/. /to/'
from=$(copier 'cd /from && find . -type f -exec sha256sum {{}} + | sort | sha256sum')
to=$(copier 'cd /to && find . -type f -exec sha256sum {{}} + | sort | sha256sum')
[ "$from" = "$to" ] || fail "the copy of $source's workspace differs from it ($from, copied $to); no sandbox was kept"
printf 'workspace digest: %s\n' "$to"
build_sandbox
trap - EXIT
observe
"#,
        target = crate::deploy::shlex_quote(target),
        egress = super::egress::EGRESS,
    )
}
