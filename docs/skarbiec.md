# Skarbiec and Stado

Skarbiec is Stado's credential service. Stado decides **where a credential is needed, which process may request it, and how the consuming service is reconciled**. Skarbiec owns **the encrypted value, the item schema, recipient encryption, consumer grants, one-field reads, rotation, audit, and ciphertext synchronization**.

The deployed fleet has one logical Skarbiec service. More than one host may carry an encrypted copy for availability or recovery, but they are not independent secret stores: Skarbiec synchronizes ciphertext between declared copies, while Stado places the service, publishes its address, selects the credential-store backend, and keeps consumers bound to the same logical service.

## The boundary

| Stado owns | Skarbiec owns |
| --- | --- |
| Registry pointers such as `account_ref` | Item ids, kinds, fields, context, tags, and recipients |
| Service placement and the service directory | Per-recipient GPG ciphertext |
| The consumer identity assigned to each Stado boundary | Grants authorizing one consumer, action, item, and optional field |
| Owner-only paths containing consumer tokens | Minting, validating, expiring, and revoking those tokens |
| Delivery to a host or service without printing the value | Decrypting an authorized field and returning only that field |
| Release, install, health, and reconciliation of the Skarbiec service | Vault writes, audit, recovery, bonds, and ciphertext synchronization |

Stado configuration and registry documents contain coordinates, never secret values. A checked-in configuration may name an item, field, consumer, token-file path, or logical service. It must not contain the value stored at that coordinate.

## One logical service, several files

A path such as `~/.stado/skarbiec.vault.json` identifies a file, not a separate product or authority. The logical service is established by three facts agreeing:

1. the Stado registry declares the Skarbiec service and its placement;
2. the Stado service directory resolves the consumer to that service;
3. the answering Skarbiec instance serves the expected item and grants.

Replica and recovery files remain encrypted. Skarbiec's bond and sync contracts move the encrypted vault document or items resealed to the destination owner; Stado does not copy plaintext between hosts. A file with the same item id but outside the declared service and synchronization topology is not an alternative source.

Use `stado host vaults` to inventory which hosts carry Skarbiec vault files. The command reports only owner and count metadata; it does not retrieve item names or values. Use Skarbiec's own sync status and audit surfaces to establish whether replicas agree.

## How a Stado process reads one field

The ordinary read path is deliberately narrower than a whole-vault or whole-item read:

1. Stado selects the configured credential store. The persisted selector is `credentials.store`; `STADO_CREDENTIALS_STORE` may request a change but cannot silently switch the running process to a different store.
2. The caller chooses one exact item id and field.
3. Its Skarbiec binding supplies three values: service URL, consumer name, and owner-only token-file path.
4. Skarbiec checks that the consumer grant authorizes the requested action, item, and field.
5. Skarbiec decrypts that field with the recipient key available to the broker and returns only the field.
6. The caller either uses the value in memory or writes it to its final owner-only runtime file. It must not put the value in argv, logs, registry documents, job documents, or machine responses.

For an operator-visible read through Stado:

```bash
stado secrets get github --field username
```

For the underlying Skarbiec CLI contract:

```bash
skarbiec get github --field username
```

Both commands keep the item id and field separate. `get` is value-bearing by definition; metadata-only inspection uses `stado secrets ls`, `stado secrets inspect-vault`, `skarbiec list`, or the dedicated status commands.

## Bindings are per boundary

A Skarbiec-backed Stado boundary is always a triple:

```text
url + consumer + token_file
```

The common configuration paths follow that shape:

| Boundary | Configuration prefix |
| --- | --- |
| General Stado secret access | `secrets.skarbiec` |
| Workload agent | `agent.skarbiec` |
| Object API verifier | `object_api.skarbiec` |
| Release API verifier | `release_api.skarbiec` |
| Machine API verifier | `machine_api.skarbiec` |
| Service API verifier | `service_api.skarbiec` |
| Rate-limit verifier | `rate_limit.skarbiec` |
| Integration verifier | `integration.skarbiec` |
| Backend messaging verifier | `backend.messaging.skarbiec` |

The equivalent environment names use the `WC_*_SKARBIEC_URL`, `_CONSUMER`, and `_TOKEN_FILE` families. Each security boundary must have its own consumer and token file. Reusing the coordinator's broad grant for a release signer, beacon publisher, object namespace, alert route, or service verifier defeats the boundary and is rejected by Stado validation where the relationship is known.

The token file is itself a bootstrap credential. It must be an owner-only regular file and must not be committed, placed in a unit definition, or passed on argv. The item value it opens stays in Skarbiec.

## Service discovery

Consumers do not carry a hand-maintained remote Skarbiec URL. Stado's service directory materializes the selected local forward as:

```text
${STADO_FORWARDS_DIR:-$HOME/.stado/forwards}/skarbiec.local
```

The file is owner-owned, not group- or world-writable, and contains exactly one bounded `https` URL or one loopback `http` URL with an explicit port. Skarbiec's credential lifecycle client fails closed when the file is absent, unsafe, malformed, stale, or points to an endpoint it cannot use.

The forward is derived from the registry's service declaration. Editing `skarbiec.local` by hand creates a second address source and is not a repair; reconcile the service directory instead.

## Fleet host accounts

A host's operating-system account is stored as a Skarbiec `host-account` item. The Stado target carries only:

```json
{
  "account_ref": "host-account-item-id"
}
```

Stado follows that pointer when an operation genuinely needs the account. The item remains typed separately from a web `login`, so browser tooling cannot enumerate a fleet host account as a form credential.

To place one field on one host without printing it:

```bash
stado host install-credential <host> <item> password <basename>
```

The destination is owner-only. The calling consumer must hold the exact field grant, and the answering Skarbiec service must have loaded that grant.

## Service runtime secrets

`stado service secret-sync` synchronizes one Skarbiec field into one variable in one managed service's runtime environment:

```bash
stado service secret-sync <service> \
  --host <target> \
  --item <item> \
  --field token \
  --variable SERVICE_TOKEN \
  --env-file ~/.config/<service>/service.env
```

The runtime file is replaced atomically and remains owner-only. The value is never placed in argv or printed. Restart is explicit with `--restart`; synchronizing a value and cycling a process are separate operations.

`stado service auth-check` verifies the installed bearer against a read-only endpoint. With `--repair`, Stado synchronizes the declared field, restarts the managed unit, and checks once more. The repair path still uses the declared item and field; it does not search other items or fall back to another credential source.

## Jobs and scheduled workloads

A scheduled Stado command may declare scoped environment secrets as coordinates:

```text
ENV_NAME=SKARBIEC_ITEM#FIELD
```

The schedule stores the coordinate. Resolution happens at execution time through the workload's own consumer grant. Provider control-plane items such as `stado-gcp`, `stado-azure`, and `stado-aws` must not be granted to a general workload agent; provider adapters receive their own identities.

Job JSON, queue state, output artifacts, status reports, and failure documents must remain plaintext-free. A missing or revoked grant fails that workload or route; it does not broaden access or substitute an ambient environment variable.

## Stado's credential commands

`stado secrets` is the operator surface over the selected credential store:

```bash
stado secrets put <item> --type <kind>    # value document on stdin
stado secrets get <item> --field <field>
stado secrets ls --json
stado secrets rm <item>
stado secrets doctor --json
stado secrets inspect-vault <vault> --json
stado secrets migrate --to <selector>
```

A backend change is a migration, not an environment toggle. If the requested selector differs from the persisted selector, normal credential access fails until `stado secrets migrate` copies the active items, verifies exact values, commits the selector, and removes the old source. A failed migration rolls back rather than allowing two active stores.

Use Skarbiec's CLI for Skarbiec-owned lifecycle operations such as consumer registration, acquisition, recipient sharing, rotation, recovery, audit, bond management, and ciphertext synchronization. Stado does not reimplement those contracts.

## Failure behavior

| Failure | Result |
| --- | --- |
| Missing item or field | The exact operation fails; Stado does not guess a replacement. |
| Consumer lacks the field grant | Skarbiec returns an authorization refusal; widening the caller is not an automatic repair. |
| Token file missing, empty, or not owner-only | Stado refuses before sending the request. |
| `skarbiec.local` missing or unsafe | Endpoint resolution fails closed; reconcile the service directory. |
| Skarbiec unavailable | Secret-dependent operations fail; unrelated queue and compute work may continue when it needs no secret. |
| Requested store differs from persisted store | All ordinary access stops until verified migration completes. |
| Replica has the same item id but is outside the declared topology | It is not used as a fallback. Repair service placement or synchronization. |
| Runtime secret is stale | `stado service auth-check` identifies the failing boundary; `--repair` performs one declared synchronization and recheck. |

No path silently reads a checked-in `.env`, a cloud CLI login, an unrelated local vault, or a provider's ambient credential chain merely because the declared Skarbiec read failed.

## Diagnostics

Use these surfaces in order:

```bash
stado capabilities --json
stado secrets doctor --json
stado host vaults
stado service directory show skarbiec
stado service auth-check <service> --host <target> --url <loopback-url> ...
skarbiec status
skarbiec doctor
skarbiec audit-verify
skarbiec sync-status
```

They answer different questions: whether the adapter exists, whether this host can open the selected store, where vault files live, where the service directory routes consumers, whether a service's installed bearer works, whether Skarbiec itself is ready, whether its audit chain is intact, and whether encrypted replicas agree.

## Release and installation

Stado is also Skarbiec's release carrier. `.wisent-release.json` declares the native build recipes; Stado builds the supported platforms, stores immutable artifacts, signs release receipts, promotes candidate and stable receipts without rebuilding, and reconciles the selected release on hosts. The running broker is never overwritten in place with a mutable `latest` binary.

Release delivery and credential rotation remain independent. A Skarbiec binary can be upgraded without changing item values or grants; a credential or consumer grant can be rotated without rebuilding Stado or Skarbiec.

## Related pages

- [Grant](primitives/grant.md) — the consumer/item/field authorization boundary.
- [Security](security.md) — what each Stado bearer opens and what never leaves a host.
- [Service directory](primitives/directory.md) — how `skarbiec.local` is materialized.
- [Configuration](configuration.md) — the complete deployed configuration surface.
- [Operations](operations.md) — service and host reconciliation.
- [Integrations](integrations.md) — support status and promotion requirements.
