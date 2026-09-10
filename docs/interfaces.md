# Primary interfaces

Every surface Stado answers on — the human CLI, the machine-readable output,
the HTTP dashboard and the desktop operator screens — and what each one is
allowed to say.


## Human CLI

`stado` is the canonical operator command. `wc` is a compatibility alias for
existing deployments.

Important command families:

```text
stado submit|status|results|cancel
stado queue pause|status|drain|resume
stado storage ls|stat|copy|verify
stado machine ...
stado artifact ...
stado host ...
stado service ...
stado web declare|deploy|route|status|list|remove
stado web edge provision|declare|status|hostnames|remove
stado dns list|set|remove
stado egress mobile serve ...
stado resources ...
stado doctor
stado capabilities --json
```

See the [CLI reference](https://stado.wisent.com/docs/cli) for arguments and exit semantics.

The graphical equivalent for bounded host-vault bearer work is **Fleet › Hosts
› selected host › Bounded vault bearer**. It can mint a new least-privilege
bearer or register an existing owner-vault item field (default field `token`).
Generated plaintext is hidden and discarded by default; **Show generated
bearer** explicitly requests the one-time raw value and presents the existing
sensitive copy control. Stored-item mode remains metadata-only. The sheet shows
target, status, grant or stored-source metadata, and Stado's refusal details.
See [Channels](https://stado.wisent.com/docs/channels#bounded-vault-bearers).

For a loaded launchd label or systemd unit that the registry does not declare,
`stado service bootout <exact-unit> --host <target> --domain system|user`
retires only that exact init-system identity. Linux bootout disables and stops
the unit and verifies it inactive without deleting its unit file; omitting the
domain preserves system-first precedence. See
[Channels](https://stado.wisent.com/docs/channels#retiring-an-undeclared-init-system-unit).

## Weles mobile egress

`stado egress mobile serve` is the data path for a Weles browser that must
leave through a tethered phone. It accepts HTTP proxy and HTTPS `CONNECT`
traffic on loopback, resolves the phone interface's IPv4 address at startup,
and binds every upstream connection to that address. It refuses LAN/public
listeners so the proxy cannot become an unauthenticated network service.

```sh
stado egress mobile serve --interface en7 --port 8781
```

The interface name comes from the target host; Stado does not guess one.
For a persistent process, deploy the same binary and arguments through the
service manager:

```sh
stado service ensure weles-mobile-egress \
  --host <weles-host> \
  --from /Users/<service-account>/.stado/bin/stado \
  --arg egress --arg mobile --arg serve \
  --arg=--interface --arg en7 \
  --arg=--port --arg 8781 \
  --reason "Weles mobile egress"
```

Weles on that host consumes `http://127.0.0.1:8781`. Stado owns process
placement, persistence, status, and logs; Weles owns which trajectory may use
the route and whether the realized exit quality is acceptable. The real-device
contract is `stado-rs/tests/egress/`: Probierz supplies a trusted phone tether,
runs the built binary, and requires the public exit to be classified as mobile
and not hosting or a public proxy.

## Web hosting

`stado web` hosts a web product on the fleet, and `stado dns` owns the
records of the zones Stado manages at their registrar. A product declares
one `web` platform in its `.wisent-release.json` whose quality and build steps
are `stado web quality` and `stado web build`, so the recipe stays declarative
and the build has one implementation for every web product.

A product that declares a `start` script is served by it; a product that does
not is a static site, and the same build stages its directory with a static
server generated into the tarball — nothing is installed on the host and
nothing is fetched at run time. `--root` names the directory when it is not
the repository root, as in `stado web build --root dist`. Build-time values go
in the platform's `secret_env` as `VAR: "item#field"` when they are
credentials, and in its `env` as literals when they are not, so a public
origin stays reviewable in the repository instead of becoming a vault entry.

```sh
stado web declare preferences-landing \
  --host charless-mac-mini --port 3210 \
  --hostname preferences.wisent.com --consumer preferences-landing-web
stado release submit preferences-landing --channel stable
stado web deploy preferences-landing
stado web route preferences-landing
```

The unit binds loopback and a declared edge host owns 443, with a Let's
Encrypt certificate per hostname. The certificate is ordered before the DNS
record is written, because a record that resolves to an edge holding no
certificate is an outage. Environment reaches the unit only through
`stado service secret-sync`, one Skarbiec field into one variable, and a
database credential is resolved for that unit's own consumer through
`stado database resolve`.

The contract, the zones it was designed against, and why the edge is a host
rather than a tunnel: [Web hosting](https://stado.wisent.com/docs/web-hosting).

## Machine JSON

Automation uses `stado machine`. Successful and failed calls use a versioned
JSON envelope with `schema_version`, `ok`, and exactly one of `result` or
`error`. Automation must not parse human tables.

The [CLI reference](https://stado.wisent.com/docs/cli) defines the noninteractive command and error
contract.

Storage request failures retain the underlying HTTP error chain, including the
actual connection, TLS, or timeout cause when the HTTP client supplies it. Release
commands, saved release failures, and the native Desktop command API receive the
same complete message rather than only `error sending request`. See
[release diagnostics](https://stado.wisent.com/docs/builds#request-failure-diagnostics).

## MCP

`stado-mcp` is a read-only stdio JSON-RPC server for AI agents. Mutations stay
behind the authenticated CLI or machine boundary.

## API listener

`stado dashboard` is the authenticated API listener over canonical state:
the product object data plane (`/api/object`), the public release channel
(`/api/release/object`), the machine API (`/api/machine/*`), the
managed-service API (`/api/service/*`), route-scoped host-health beacons,
and the shared rate limiter. It serves no HTML page — the operator
workspace is Stado Desktop. Local onboarding binds the listener to
loopback. Remote exposure additionally requires authenticated deployment
configuration and a trusted reverse proxy.

This listener is not a human Supabase API. Object, release, machine, service,
host-health, rate-limit, and enrollment routes keep their route-specific
application, workload, or invitation credentials; they never turn a workload
token into a Wisent login or infer an organization. The repository contains no
remote human organization action served by this listener.

Three enrollment routes sit beside that surface and outside its operator
identity, because the machine being invited has one credential and it is not an
operator's: `GET /api/fleet/invite/key` and `POST /api/fleet/join` authorize on
an invitation token alone, `GET /join.sh` serves the script and authorizes
nothing. None of the three can write the registry — they hand out the fleet's
public key and record a pending request, and the registry write happens later
under operator authority in `stado fleet approve`. All three serve the `invite`
method's one-line mode, and therefore only a machine that can reach this
listener — which a loopback binding does not allow; the method's offline mode
exists so that adding a machine never depends on them. See
[the invite endpoints](https://stado.wisent.com/docs/cli#invite-endpoints-on-the-dashboard) and
[the control-point check](https://stado.wisent.com/docs/cli#the-control-point-check).

## Desktop operator screens

Stado Desktop is optional — the CLI stays canonical. Its screens invoke typed
product operations and render their retained results. Where Desktop uses an
authenticated dashboard API, including service convergence and storage-root
reconciliation, the API and CLI call the same product implementation; provider
APIs and credential values do not get a second implementation in the app.
Build it with `swift build --package-path desktop/StadoDesktop`; signing and
publication of the app go through the desktop publisher documented at
https://stado.wisent.com/docs/publisher. The screenshots below are live
reads of the Wisent fleet.

The deployment registry is the remote human organization HTTP surface in this
repository. Stado Desktop sends every registry request to the canonical
Supabase project at `https://alvaewvbyxpgwdpugnxy.supabase.co` with both
`Authorization: Bearer <Supabase JWT>` and
`X-Wisent-Organization-ID: <uuid>`. Supabase derives the user only from the JWT
and calls `authorize_organization` for the header organization. Members may
read that organization's deployments; only owners and admins may create,
update, or delete them. Infrastructure targets remain scoped to the JWT user.
Request bodies contain neither user IDs nor roles, and there is no per-user or
per-role deployment-grant fallback.

The table definitions, RLS policies, and migrations are owned exclusively by
[`wisent-supabase-oko`](https://github.com/wisent-ai/wisent-supabase-oko).
Stado consumes that deployed schema and keeps no product-local `supabase/`
tree.

![Stado Desktop Releases screen: brama on control-host blocked, its blockers, the candidate's stderr tail, and the quarantined digest the registry desires](desktop/StadoDesktop/docs/screenshots/releases.png)

*Releases — find out why a rollout never finishes: the verdict and blockers
`stado release doctor` reached for every product target, the host's own
software report, the tail of the candidate's stderr off the host
(`stado release logs`), and the digests the host refuses to roll out again with
the desired one first (`stado release quarantine list`). Clearing a digest is
done here, with a typed reason, and starts nothing by itself.*

`stado release active-binary <product> --json` is the machine-readable answer
for an executable another service should launch. It accepts only the observed
active release whose live process tuple, exact stable proxy route, immutable
manifest identity, and policy-derived executable agree; desired or quarantined
bytes merely left on disk are never returned. A declared product/target with no
observed active release is unavailable rather than eligible for a legacy-path
fallback.

![Stado Desktop Hosts screen: two hosts claiming no work, their blockers and disk policy, and the inspector for control-host](desktop/StadoDesktop/docs/screenshots/hosts.png)

*Hosts — find out why a host is claiming no work: the blockers its own agent
publishes, the free space against the watermark the cleanup policy enforces,
and the age of its capacity report (`stado host gates`). Disk reclamation
starts from the inspector and previews before it deletes
(`stado host reclaim`).*

![Stado Desktop Services screen: declared units per host with their running binary, two marked as serving replaced code, and the inspector for the drifted skarbiec unit](desktop/StadoDesktop/docs/screenshots/services.png)

*Services — find out what the fleet is actually running: each declared unit's
state, the program it declares, the binary the process is really executing and
whether the two agree (`stado service converge`), plus the product processes no
unit owns at all (`stado service list --unowned`).*

Services report and apply use the authenticated `GET` and `POST`
`/api/service/converge?target=<host>[&binary=<name>]` API, which shares the
implementation of `stado service converge`. A completed request preserves the
product's `exit_code` and full `report` even on failure. Desktop keeps the
complete receipt through refresh rather than reducing it to a status label.
Settings → **Registry API access** binds a raw client token file to the exact
source endpoint; a Wisent account token is not a registry credential.
`STADO_REGISTRY_API_URL` and `STADO_REGISTRY_API_TOKEN_FILE` override those
saved settings. See the canonical [Desktop](https://stado.wisent.com/docs/desktop)
and [configuration](https://stado.wisent.com/docs/configuration) pages.

The server's independent Skarbiec verifier grant can be created without
transferring its bearer through the operator's terminal:

```bash
stado host vault-token-mint charless-mac-mini stado-registry-api-verifier \
  --capabilities read:stado-desktop-registry-api#token \
  --audience skarbiec \
  --token-file-name stado-registry-api-verifier-skarbiec-token --json
```

`--token-file-name` creates an owner-only file under the target's `.stado`
directory when absent and reuses its exact bearer when present. Skarbiec
still owns the grant, capability changes and expiry. If minting fails, the
file remains available for the same command to resume. The Desktop client
bearer is a separate `stado-desktop-registry-api/token` item, not this verifier
grant; its declared actions must include `converge-read` and `converge-apply`
for both Services operations.

*Cloudflare routes — list, inspect, add, update and remove hostnames on a
declared Cloudflare Tunnel without leaving Stado Desktop. The screen compares
tunnel ingress with exact proxied CNAMEs, reports connector connections
separately from the deliberately unprobed origin, and reads nonsecret item ids
with `stado credentials ls --json`. Add/update exposes every option of
`stado cloudflare route-tunnel`; removal deletes matching tunnel DNS before
ingress and preserves the shared connector, service and credential. Every
mutation quotes its exact CLI invocation and requires a confirmation.*

