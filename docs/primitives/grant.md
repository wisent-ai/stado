# Grant

How does a Stado process get a credential, and why does none of them have a
cloud login? A grant is a narrow Skarbiec consumer identity: Skarbiec owns the
credentials, and each Stado process consumes exactly the items its grant
names.

## What it is

Skarbiec is the separate credential service, reached over loopback HTTP or a
TLS-protected remote endpoint, enforcing scoped consumer grants
(`stado-rs/src/skarbiec/mod.rs`). A grant is a consumer name plus an opaque
token in an owner-only file; a token file readable by group or other users, or
empty, is refused before any request is made.

Grants are deliberately plural and narrow. Each verifier is an auth boundary:
it enforces its exact consumer name and a token file distinct from every other
grant, and it never routes through the credential-store selector
(`stado-rs/src/skarbiec/verifiers.rs`):

| Consumer | Scope |
|---|---|
| `stado-control-plane` | The coordinator's default consumer, with its owner-only grant file ([cli](../cli.md)). |
| `stado-host-health-beacon` | The beacon publisher: resolves only `stado-host-health-api/token`, nothing else ([operations](../operations.md)). |
| object-api verifier | Namespace-scoped product object bearers only; its token file must be distinct from the coordinator grant, enforced at construction. |
| release-signing reader | One capability: `read:stado-release-signing#private_key`. Signing material is the last thing that should travel on a broad grant. |
| alert key reader | The one credential the alert path needs. Paging is the last thing that should need a broad grant. |

The narrowness is load-bearing: when `release submit` read the signing key
through the coordinator grant, the vault refused with `403 consumer not
authorized` — the policy was right and the caller was reaching for the wrong
identity (`stado-rs/src/skarbiec/verifiers.rs`).

Secrets stay where they are minted. When a managed service's bearer is synced
from a Skarbiec item, the item is read on the host by the host's own Stado
identity, so the value never travels on the operator's channel and the
operator's consumer needs no grant for it
(`stado-rs/src/deploy/service.rs::sync_service_item_secret`).

## Who declares it

The fleet provisions least-privilege consumers in Skarbiec and configuration
names them — only non-secret routing metadata is checked in
([configuration](../configuration.md)): the Skarbiec origin, the exact
consumer name, and the path of the owner-only token file
(`STADO_HOST_HEALTH_SKARBIEC_URL` / `_CONSUMER` / `_TOKEN_FILE` for the beacon
publisher). A workload-agent grant contains only the provider-neutral
application items in `agent.skarbiec.items`; it must never contain
`stado-gcp`, `stado-azure`, or `stado-aws`, and such a grant fails profile
validation ([configuration](../configuration.md)).

One deliberate exception: a backend's own bootstrap credential stays outside
the store, because putting the grant needed to unlock a manager inside that
same manager would be circular ([configuration](../configuration.md)).

## Who observes it

`stado host inventory` reports the Skarbiec vault files under `$HOME/.stado`
as metadata only — name, size, mode, owner-only — and never opens a vault: not
to read a byte of ciphertext, not to count items. That is a boundary, not an
oversight ([cli](../cli.md)). A missing, unreadable, or over-broad grant on
the beacon path leaves the prior beacon untouched and returns failure
([operations](../operations.md)).

## Where it lives

Owner-only token files on the consuming host, for example
`~/.stado/host-health-beacon-skarbiec-token` and
`~/.stado/control-plane-skarbiec-token`. Non-secret routing lives in
`/etc/stado/host-health.env` on Linux and the launchd template on macOS
([operations](../operations.md)).

## Commands

```bash
stado secrets ls
stado host publish-beacon FILE
stado host inventory <registry-target>
```

All `stado secrets` CRUD, provider reads, and scoped verifier reads use the
selected credential store ([configuration](../configuration.md)); flag detail
lives in [cli](../cli.md).

## Not to be confused with

- **An ambient cloud credential.** There is no cloud CLI, provider SDK, direct
  bucket URL, ambient credential, or cross-backend fallback in the beacon
  writer, and no bootstrap, health, recovery, or release path invokes
  `gcloud`, `gsutil`, or `az` ([operations](../operations.md),
  [configuration](../configuration.md)). Provider access belongs to the
  enabled adapter's exact plugin identity, never to a grant.
- **A Skarbiec item.** The item is the credential; the grant is the scoped
  permission to read named item fields. Rotating one does not rotate the
  other.
- **A fleet channel key.** Host channels use registry-owned ed25519 keys
  managed by `stado fleet key`; a grant opens Skarbiec, not a host
  ([cli](../cli.md)). See [security](../security.md) for the whole boundary.
