# Local inference

How do you serve a model from a fleet GPU and route traffic to it? `stado
inference` plans, deploys, routes and operates local OpenAI-compatible
inference: a digest-pinned vLLM container on a registered GPU host, addressed
through logical routes on the Brama gateway.

This plane is being replaced by the service declaration contract: a model
server is a service like any other, declared once with `stado service declare`
and deployed with `stado service deploy`. It keeps working while its
declarations migrate; add nothing new to it. See
[service](primitives/service.md).

Flag-by-flag listings live in [cli](cli.md); the registry section and worked
lifecycle live in [configuration](configuration.md).

## What a deployment is

The optional top-level `inference` section of the canonical registry is the
single desired-state and routing catalog for Stado-managed vLLM.
`gateway_target` is the registered host running Brama; deployments run on
registered local GPU targets and expose their OpenAI-compatible endpoint only
on a Tailscale IPv4 address.

A deployment pins what it runs: the image is a digest-pinned vLLM image
(`repository@sha256:digest`) and the model revision is an immutable Hugging
Face commit SHA, so neither the runtime nor the weights can be replaced
silently. On the host it is one Docker container named
`stado-inference-<name>`, supervised by Docker's `unless-stopped` restart
policy, with the Hugging Face cache mounted from a persistent host directory
(`--cache-dir`; set it when the target's home filesystem is not the intended
model volume).

## Planning and applying

The lifecycle is deliberately two-step and generation-fenced:

```bash
stado inference plan chat-primary \
  --host gpu-host \
  --image 'vllm/vllm-openai@sha256:<image-digest>' \
  --cache-dir /srv/stado/inference/chat-primary \
  --model 'example/model' \
  --revision '<model-revision>'
stado inference apply <plan-id>
```

`plan` inventories the host, requires Docker, NVIDIA tooling, and a live
Tailscale address, then saves an immutable plan bound to the current registry
digest, locally under `~/.stado/inference-plans/<id>.json`. `apply` executes
one persisted plan only if that registry precondition still matches: it
installs the container, waits for an authenticated readiness probe, and only
then commits the deployment to the registry. A failed runtime, readiness
check, or registry compare-and-swap restores the prior runtime.

Install refuses a host whose GPU has an unmanaged active compute process, an
endpoint port already in use by anything other than this deployment's own
container, and a host that already carries another inference reservation.

`--gpu-mode` decides who owns the board. `exclusive` (the default) keeps the
GPU reserved for inference. `yieldable` makes the local Stado agent the
lifecycle owner: it pauses the inference container when an eligible GPU job is
queued, advertises the released capacity, and resumes inference only after
queued and active GPU work has cleared. There is no timeout-based eviction,
and the route's ordered provider fallback remains available while the local
container is yielded.

## Routing

Workloads name a route alias, never a host. `route set` atomically updates one
logical route:

```bash
stado inference route set example-client/chat/primary \
  --to chat-primary \
  --fallback openai/gpt-4.1-mini \
  --expected openai/gpt-4.1-mini \
  --gateway gateway-host
```

Every change requires `--expected` as a compare-and-swap precondition
(`absent` for a new route). The command probes the destination first, stages
an owner-only route snapshot on the gateway, compare-and-swaps the registry,
and then atomically commits the snapshot. Brama reloads that file per request,
so cutover needs no backend restart. `--gateway` names the registered host
running Brama and is required on the first managed route.

Ordered `--fallback` destinations are attempted when the primary fails; an
external provider fallback (see [providers](providers.md)) preserves the same
model contract while local inference is unavailable. A non-ready `yieldable`
deployment is accepted only as the primary of a route with at least one
ordered fallback; an `exclusive` primary and every local fallback must be
ready, so an unavailable deployment cannot be published as a route's only
destination.

## Reading health

| Command | What it answers |
|---|---|
| `stado inference list` | Declared deployments, without contacting hosts. |
| `stado inference status NAME` | The deployment's state from the latest host beacon. |
| `stado inference doctor NAME` | Runtime, GPU, endpoint and authentication, inspected on the host. |
| `stado inference verify NAME` | One minimal authenticated OpenAI-compatible completion, end to end. |
| `stado inference logs NAME` | The deployment's log tail (default 255 lines) over the managed host channel. |
| `stado inference plan-logs PLAN_ID` | Logs for a runtime whose plan has not committed, through the same channel. |

The host-side readiness probe is the same fact `doctor` and `apply` rely on:
the `stado-inference-<name>` container must report `running`, and an
authenticated request to the endpoint's `/v1/models` must succeed. `verify`
goes one step further and posts a real `/v1/chat/completions` request.

## GPU contention

If `plan` or `apply` reports an unmanaged GPU workload, inspect it through the
same target-scoped host channel instead of an ad hoc SSH session:

```bash
stado inference blockers --host gpu-host
stado inference release --host gpu-host --identity <PID:START_TICKS>
```

`blockers` reports the executable, owner, VRAM use, cgroup, and an identity
made from both PID and `/proc` start ticks. `release` refuses a stale
identity, sends `TERM`, and waits for exit; `--force` only escalates that same
verified process to `KILL`. It never accepts a bare PID.

## Teardown

`retire` stops and forgets a deployment. It refuses while any primary or
fallback route still selects the deployment, and it retains the model cache
unless `--purge-cache` is explicit. On the host it removes the container, the
deployment's reservation, and its stored credential material.

`rollback NAME` reinstalls the previous deployment generation.

`abort PLAN_ID` cleans up after a cancelled or failed pre-commit plan, which
can leave a runtime or root-owned model cache without a registry deployment.
It never changes the registry: it stops only the runtime described by the
immutable local plan, removes its cache through the pinned container runtime
with `--purge-cache`, and consumes the plan after successful cleanup.

## Credential

The deployment and Brama share one centrally stored credential item,
`provider:local-openai`, containing a non-empty `token` field:

```bash
stado inference init-credential
```

This generates and stores the bearer in Skarbiec without printing it, and
refuses to overwrite an existing item. Deliberate rotation must be coordinated
with runtime replacement; never place the token in argv or registry data. See
[security](security.md).
