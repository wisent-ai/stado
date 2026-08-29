# Channels

A Stado channel is a declared connection between two managed parts of the fleet. It is not an SSH session, a copied token, or an operator-maintained tunnel. The registry names both endpoints, Stado checks the relationship, and the host channel carries any repair without returning secret values to the caller.

## The four boundaries

| Boundary | What crosses it | Source of truth | Commands |
|---|---|---|---|
| host channel | fixed programs and scripts over the target's declared transport | registry target plus host key | `stado host exec`, product-specific `stado host` commands |
| service directory | service name to endpoint and consumer relationship | registry service declarations | `stado service directory show`, `stado service directory connect`, `stado service verify` |
| object authorization | object URI plus action; bearer remains local to the caller | `object_api.namespaces` and their Skarbiec items | `stado storage stat|get|put|ls|rm`, `stado host reconcile-object-verifier` |
| workload secrets | one exact item field delivered to one service environment | Skarbiec grant plus managed-service declaration | `stado service grant-sync`, `stado service auth-check`, `stado service secret-sync` |

These boundaries are separate. A host may answer while a service relationship is wrong. A service may listen while its bearer is stale. The object API may be live while its verifier cannot read namespace credentials. Health therefore means the final operation succeeds, not merely that a process or TCP port exists.

## Host channel

The host channel resolves the target from the canonical registry, checks its pinned host identity, and runs a fixed operation through Stado. Anything absent from the command allowlist belongs in a checked-in Stado operation; it does not justify raw SSH.

Use `stado host exec <target> <allowlisted-command>` for read-only diagnostics. Mutating workflows use their owning commands, such as `stado service file-sync`, `stado service secret-sync`, `stado release apply`, or `stado host reconcile-object-verifier`. Secret values are read and written on the target; they never appear in the remote argument list or command result.

## Service directory

The directory joins a producer, its endpoint, and declared consumers. `stado service directory show <service>` displays the resolved relationship. `stado service directory connect <service> --consumer <consumer>` establishes a declared connection. `stado service verify <service>` exercises reachability from the declared consumers instead of treating the producer's loopback listener as fleet reachability.

A directory declaration is not proof of a live route. Verification records which consumer reached which endpoint and why a refusal occurred.

## Object authorization

The object API authorizes an action in two stages:

1. the caller bearer selects one namespace policy;
2. the object API verifier uses its own host-local Skarbiec grant to read the exact namespace credential items declared in `object_api.namespaces`.

The verifier bearer lives in `WC_OBJECT_SKARBIEC_TOKEN_FILE` on the object API host. To restore its grant without moving the bearer off that host:

```console
stado host reconcile-object-verifier <target> --json
```

The command derives the item set from `object_api.namespaces`, asks Skarbiec on the target to bind the existing bearer to that exact set, and reports item names and expiry only. It never prints the bearer.

Release publication has a separate verifier and per-product policy set. Reconcile
only the product being published:

```console
stado host reconcile-release-verifier <target> --product <product> --json
```

The command resolves `<product>` through `release_api.publishers`, reads only that
controller-owned publisher item, writes only its target-local shadow, and adds
only its `token` read to the existing verifier grant. Other product capabilities
and shadows are preserved and are never rotated by this operation. Release
preflight proves the caller credential with an authenticated operation under the
same product prefix. A public `releases/` stat proves neither boundary.

Use `stado storage stat <stado-uri> --json` as the smallest final check. `present` and `absent` are both authoritative answers. `503 object authorization unavailable` means the verifier boundary failed; it is not evidence that the requested object is absent.

## Workload grants and service authentication

`stado service grant-sync` binds an existing owner-only token file on one host to an exact consumer and capability set. Skarbiec reads the bearer locally and stores only its digest:

```console
stado service grant-sync <service> \
  --host <target> \
  --consumer <consumer> \
  --capabilities '<item>#<field>:read' \
  --token-file '$HOME/.stado/<consumer>-skarbiec-token' \
  --json
```

`stado service auth-check` then sends that bearer from the host to a read-only loopback endpoint. With `--repair`, it synchronizes the named item field into the managed environment, restarts only the declared unit, and checks the endpoint again. `--take-over-listener` is a separate, explicit recovery for an unmanaged process occupying the declared port.

## Failure ownership

| Result | Meaning | Repair owner |
|---|---|---|
| target absent from registry | the host channel has no declared destination | registry declaration |
| host identity mismatch | transport reached a different machine | host enrollment |
| directory has no consumer relationship | route was never declared | service directory |
| `401` or `403` from final endpoint | caller grant or policy rejected the operation | caller/workload grant |
| `503 object authorization unavailable` | object API verifier cannot read namespace credentials | object verifier reconciliation |
| endpoint answers but final state is unchanged | process health passed, product operation failed | owning product workflow |

Retries do not repair a declaration or grant. They are appropriate only after a retryable transport failure where the same declared operation remains valid.

## Tests

Repository tests live under `stado-rs/tests/<area>/main.rs` and drive the real
`stado` binary. The directory listing is the inventory; this page does not copy
it, because a hand-copied list rots into names that do not exist. The journeys
those tests answer for are declared in the Probierz manifest for app `stado`.

The channel boundaries above are defended by:

- `tests/channel/` — the public release channel: stat, download, digest verification, and execution of one immutable native release through the public HTTPS origin;
- `tests/service/` — managed service grant synchronization, authentication checks, and service transitions;
- `tests/recovery/` and `tests/recovery_release/` — host-channel recovery and signed release recovery;
- `tests/domain/`, `tests/link/`, `tests/removefile/` — host-channel identity, declaration, and guarded mutation.

Probierz is the execution and evidence boundary when it is operational. The test source remains in this repository. A passing parser, dry run, mock server, or successful process start is not channel evidence; the journey must observe the promised final state in the real connected component.
