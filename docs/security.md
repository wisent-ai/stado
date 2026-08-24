# Security

Where does trust start and stop in a Stado deployment? This page names each
credential class and what it opens, the shape of the host channel, what never
leaves a host, how credentials rotate, and what enrollment actually proves.
Command surfaces live in [cli](cli.md); this page is the boundary map.

## Tokens and what each opens

There is no global dashboard bearer. Every API bearer is verified against a
value held in Skarbiec, the separate credential service, and every comparison
is constant-time. A missing or empty verifier item is an error the route
reports as a redacted 503 — never access.

| Bearer | Verified against | Opens |
|---|---|---|
| Object API token | The configured namespace's own Skarbiec item | Object actions (`get`, `put`, `stat`, `list`, `delete`) inside that one namespace, and only on keys matching the namespace's prefix allowlist. An out-of-scope key or action is unauthorized before the token is even compared. |
| Release publisher token | A per-product Skarbiec item resolved from the exact product prefix inside `stado://releases` | Immutable release writes for that product prefix. The former global object token is never consulted on this route. |
| Service deployer token | A per-service Skarbiec item resolved from the service name and action | The service API's `status` and `restart` routes for that service and action, nothing wider. |
| Host-health beacon token | The bearer stored as `stado-host-health-api/token`, and nothing else | `PUT /api/host-health` — one route. Machine clients are authorized separately through exact client policies. |

Sources: `authorize_object`, `authorize_release`, `authorize_service`, and
`authorize_host_health` in `stado-rs/src/dashboard/mod.rs`; the namespace
prefix-allowlist model in `stado-rs/src/config.rs`.

Every active product namespace must hold explicit object-gateway credentials;
`releases` is intentionally absent from that list because it stays on the
dedicated public GET-only release route (`config.rs`,
`ACTIVE_OBJECT_NAMESPACES`). The gateway authorizes a write by matching its
key against the namespace's prefix allowlist — a write under a prefix no
namespace declares is refused with a 401, which is why the whole autonomy
layer is rooted under `state/` ([operations](operations.md)).

The beacon writer holds the narrowest grant in the fleet: the dedicated
`stado-host-health-beacon` Skarbiec consumer resolves only
`stado-host-health-api/token`, an unreadable or over-broad grant is a
failure, and the opaque grant sits owner-only at
`~/.stado/host-health-beacon-skarbiec-token` ([operations](operations.md)).

## The host channel

Every host mutation rides one shared SSH channel whose option set is derived
from a single source (`deploy/host_reboot.rs::ssh_reboot_argv`) rather than
re-typed per command, so `BatchMode=yes`, `ConnectTimeout` and
`StrictHostKeyChecking=accept-new` cannot drift between the `stado host` and
`stado service` commands. The remote program is fixed and narrow, it reports
through the tab-delimited `STADO_*` marker protocol, and registry data never
becomes a shell fragment (`stado-rs/src/deploy/service.rs` module header).

`stado host exec` is the only operator-worded remote execution, and it is an
allowlist, not a shell. Three independent barriers stand between the
operator's words and the host (`stado-rs/src/deploy/host_exec.rs`):

1. **Character rejection.** Every word must consist only of characters no
   shell treats specially; `;`, `|`, `&`, `$`, backtick, quote, newline,
   redirection and glob are refused by name first.
2. **Exact allowlist match.** The joined words must equal one approved entry
   exactly — no prefix match, no extra arguments, no operator-supplied path,
   because a command that took a path could read `~/.ssh/id_ed25519`.
3. **Fixed argv.** What runs is the matched entry's own compile-time argv of
   absolute paths. The operator's words select an entry; they never become
   part of the command line.

Extending the allowlist is a commit: each `ApprovedCommand` entry carries the
fixed `argv` and a `why` field justifying unattended execution as the
registry-managed login user. An entry without a defensible answer there does
not belong in the table. Every entry is read-only — the allowlist cannot read
a file.

## What never leaves a host

Secret values are moved onto hosts or minted on them; they are not read back.

- `stado service secret-sync` in its item form resolves the Skarbiec item on
  the host, by the host's own Stado identity, so the value never travels on
  the channel and the operator's consumer needs no grant for it
  (`deploy/service.rs::sync_service_item_secret`).
- The value-carrying form reads the secret through the isolated
  service-verifier grant and carries it in the SSH request body; it is never
  printed or placed in argv (`stado service secret-sync --help`). Existing
  unrelated variables in the env file stay on the host and never cross back
  to the operator.
- `stado service auth-check` with an item reference resolves the bearer on
  the host and reports only the HTTP outcome; the bearer itself never leaves
  the host (`deploy/service.rs::check_service_item_bearer`).
- Consumer grants can be reminted against the host's own authoritative vault
  and landed owner-only at their token path; the value never crosses the
  channel (`deploy/service.rs::remint_consumer_grant_on_host`).
- SSH channel keys: `stado fleet key generate` prints the public half only;
  the private half never leaves the credential store ([cli](cli.md)).

## Rotation

Rotation is a first-class operation, not a re-enrollment:

- **Application credentials** live in Skarbiec and are managed with
  `stado credentials`: `put` reads from STDIN, `get` is the one subcommand
  that renders a value, `migrate` moves every credential to a new backend and
  commits the selector, and `mint-acquisition-token` mints a request-only
  bootstrap token directly into an owner-only file
  (`stado credentials --help`).
- **Service runtime secrets** rotate with `stado service secret-sync`, which
  atomically replaces one variable in the unit's env file (a mode-600 atomic
  rename on the host) and restarts the service only when asked with
  `--restart` (`stado service secret-sync --help`, `deploy/service.rs`).
- **Channel keys** rotate end to end with `stado fleet key rotate`, with
  rollback on failure ([cli](cli.md)).

The rotation blast radius is the namespace boundary itself: the entitlements
rotator holds its own object namespace (`entitlements-rotator` in
`config.rs::ACTIVE_OBJECT_NAMESPACES`), so rotating one product's credential
changes one Skarbiec verifier item and touches nothing another namespace
uses.

## Enrollment trust

A machine is not trusted because it answered SSH. Enrollment is
agent-attested ([examples](examples.md), `examples/fleet.md`): SSH
reachability and hostname discovery create only a non-routable
`provisioning_targets` entry, and the machine enters `targets` and a fleet
only after the installed agent publishes fresh capacity with its Stado
version. An unreachable host, failed installation, missing attestation, or
registry conflict removes the provisioning entry and leaves no registered
target. Once the registry catalog sets `require_agent_attestation`, the
validator also rejects every local target without a valid receipt.

The join API is equally narrow: both invite-token routes are authorized by
the invite token alone, and `POST /api/fleet/join` writes a pending request —
it creates no registry entry and cannot modify one ([cli](cli.md)).

## The money boundary

Nothing in Stado links a billing account or raises spend by itself. Stado's
billing credentials are readers — the Azure billing principal carries Billing
account reader plus subscription Billing Reader, resolved from Skarbiec with
exact per-field capabilities ([cli](cli.md)) — and the autonomy loop carries
a budget guard that blocks new cloud placement outright when the cost
forecast exceeds the configured budget (`stado-rs/src/coordinator.rs`,
[autonomy](autonomy.md)). Spend follows submitted work under the scheduler's
own cost limits ([costs](costs.md)); it is never a side effect of holding a
token.
