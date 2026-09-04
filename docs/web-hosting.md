# Web hosting

Stado takes a Node web product from its repository to a public hostname: the
release pipeline builds it on a fleet host, the registry runs it as a managed
unit with its environment delivered from Skarbiec, and a Stado-owned DNS record
plus a Stado-owned TLS edge publish it. Nothing in that path belongs to a
third-party build-and-host platform.

This page is the contract. It states what the fleet already had, what was
added, why the ingress works the way it does, and the exact command sequence an
operator runs.

## What the fleet already had

Three pieces existed before web hosting and are reused unchanged.

**The release pipeline.** `.wisent-release.json` in a product repository
declares platforms, quality gates, a build command, and the files to stage
(`stado-rs/src/release_pipeline.rs`). `stado release submit` snapshots the
source, picks a builder whose registry `release_platform` matches the recipe's
`runner_platform`, runs the gates and the build there, publishes the staged
bytes into a channel, and promotes them. Platform keys are free names; only
`runner_platform` is constrained, to `darwin-arm64` or `linux-amd64`.

**The service registry.** A unit is a `ServiceDeclaration` — an immutable
source (`artifact`, `sha256`) and a run spec (`program`, `args`, `env`) — held
in the canonical registry beside its verification descriptor and its consumers
(`stado-rs/src/declaration.rs`, `stado-rs/src/service_resolution.rs`).
`stado service deploy` renders it to a launchd plist or a systemd user unit and
bootstraps it; `stado service secret-sync` puts one field of one Skarbiec item
into one environment variable in the unit's runtime env file, over the host
channel, never on a command line; `stado service grant-sync` reconciles the
unit's Skarbiec consumer grant.

**The database plane.** `stado database resolve <db> --consumer <consumer>`
answers with the Skarbiec item that carries the credential, and refuses a
consumer the declaration does not list (`stado-rs/src/cli/database.rs`). The
value never leaves Skarbiec through this path; the item name does.

## What the zones and the credentials actually are

Everything about the ingress follows from measurements taken on 2026-09-02.

**The zones are split between two registrars, and the ones that matter are at
Namecheap.** `dig +short NS <zone>` puts `wisent.com`, `wisent.ai`,
`needher.ai`, `aiwritecheck.com` and `undetectabletext.com` on
`dns1/dns2.registrar-servers.com` — Namecheap. It puts `wisent-app.com`,
`aiwisent.com`, `getwisent.com`, `trywisent.com`, `wisentai.com`,
`wisentplatform.com`, `bobloo.com`, `pol-acc.com`, `tour-bot.com`,
`lukaszbartoszcze.com` and `downloadreal.com` on
`gabriel/galilea.ns.cloudflare.com`, `controlai.org` on `love/ned.ns.cloudflare.com`,
and `alpha2.ai` at GoDaddy. Both Preferences hostnames live in `wisent.com`.

**Vercel is only the custom-domain front; the fleet already serves the bytes.**
`curl -sI https://preferences.wisent.com/` answers 200 with `server: Vercel`
and an `x-vercel-id` header. The `brama-ingress` and `vercel-ingress` projects
are the same code — the `vercel-ingress` root of `wisent-ai/brama`, whose
`vercel.json` is a catch-all rewrite to
`https://charless-mac-mini.tail6443b3.ts.net/:path*`. So Brama is already
published over public HTTPS by the mini itself, and Vercel contributes exactly
one thing: a certificate for a `wisent.com` name.

**No fleet host has a public address.**
`stado host exec ubuntu-server-rtx-pro-6000 -- tailscale netcheck` reports
`IPv4: yes, 24.23.232.108:56883`, and `curl -s -4 https://api.ipify.org` from
the operator's laptop answers the same address: every host sits behind one
residential connection. Inbound TCP to 80 and 443 on that address times out,
and `PortMapping:` in the same report is empty.

**The fleet's only public entrance is a Tailscale Funnel.**
`stado host exec charless-mac-mini -- tailscale funnel status` reports Funnel
on for `https://charless-mac-mini.tail6443b3.ts.net` on ports 443, 8443 and
10000, each forwarding to a loopback origin.

**There is no Cloudflare API credential.** `platform-admin-cloudflare` carries
`username` and `password` — a console login. `platform-cloudflare-bobloo-tunnel`
carries `account_id`, `token`, `tunnel_id` and `tunnel_name`, and its `token`
is a 180-character `cloudflared` tunnel token: presented to the Cloudflare API
as a bearer it is refused with code 6111, `Invalid format for Authorization
header`. `stado cloudflare` requires an `--api-credential` item carrying
`account_id` and `api_token`, and no such item exists.

**Azure administration is unblocked.**
`stado azure unusual-activity diagnose` on subscription
`9ae7cfa4-93e4-44f6-8f4d-5cea670e22bd` reports
`active_unusual_activity_denies: 0` and
`resolution: no_active_unusual_activity_deny`, through the
`stado-azure-operator` credential. `stado capabilities` lists `compute`/`azure`
as `implemented` (`providers::azure::AzureProvider`), and the Azure grant is
USD 100,000 valid to 2028-05-06 with its spending limit removed.

## Why the ingress is a Stado edge host, not a tunnel

A public `https://<hostname>` on the operator's own domain needs two things in
the same place: a route the public internet can reach, and a certificate for
that exact hostname. The fleet has the route only through Tailscale Funnel, and
Funnel cannot supply the certificate. Three mechanisms can, and they were
weighed against what is actually true above.

**Tailscale Funnel with a Namecheap CNAME — ruled out, verified.** Tailscale
states the limit itself: Funnel can only use DNS names in the tailnet's own
domain (`tailnet-name.ts.net`). A CNAME from a custom name to the MagicDNS name
reaches the Funnel ingress and then fails the TLS handshake, because the
ingress routes by SNI and holds no certificate for the custom name
(tailscale/tailscale#16478, and the open request #11563 to allow custom domains
at all). This is the mechanism `brama-ingress` works around by putting Vercel
in front, and it cannot be fixed on our side.

**A Cloudflare Tunnel — kept, but not for `wisent.com`.** `cloudflared` dials
out, so no inbound port is needed, Cloudflare terminates TLS with a certificate
it issues, and the hostname is a proxied `CNAME` to
`<tunnel_id>.cfargotunnel.com`. Stado already speaks exactly this in
`stado cloudflare route-tunnel`, and `cloudflared` is already running on the
mini. But Cloudflare issues that certificate only for a hostname in a zone it
serves, so a `wisent.com` hostname would mean moving that zone's nameservers
off Namecheap and re-creating every record it holds, the Google Workspace MX
records included. Delegating only the `preferences.wisent.com` subtree avoids
the apex but needs a Business plan, so it trades a nameserver move for a
monthly bill. **Moving a zone's nameservers is the operator's decision and is
not taken here.** This edge stays implemented and is the right one for the
eleven zones already on Cloudflare.

**A Stado edge host with a public address — chosen.** One small Linux VM,
provisioned by Stado through the Azure provider it already implements, joined
to the fleet like any other host and to the tailnet with it. It holds the
public IPv4 address, terminates TLS for the product hostname with a Let's
Encrypt certificate, and forwards over the tailnet to the unit on whichever
fleet host runs it. `wisent.com` stays at Namecheap; the only change to that
zone is the product hostname's own A record, written by `stado dns`, which
reads the whole zone, merges one name, and writes it back.

That choice wins on the facts rather than on taste. It touches no nameserver
and no MX record, so it needs no decision that is not already the operator's
to delegate. It works the same for every zone in the inventory — five at
Namecheap, eleven at Cloudflare, one at GoDaddy — where the tunnel works only
for the Cloudflare ones. It removes the last thing Vercel does for the fleet,
for all 69 projects, with one mechanism. And the certificate is ours: Let's
Encrypt issues it to a host we own, rather than an edge provider issuing it on
our behalf.

Its cost is one VM. A `Standard_B2pts_v2` (2 vCPU, 1 GiB, ARM64) in `westus2`
is about USD 15 a month; the reverse proxy is the only thing that runs on it.
That is spend against the existing Azure grant — USD 100,000 valid to
2028-05-06 — and not a new purchase or a raised limit.

## What was added

**`stado web`** — the product-level capability. It owns the shape of a web
product: which release artifact it runs, on which host and port, under which
Skarbiec consumer, with which environment, behind which hostname.

**`stado dns`** — the registrar. Namecheap's `setHosts` call replaces a whole
zone, so a record cannot be changed without re-sending every other record in
it; that is why `wisent.com`'s records were written by a script inside a
product repository. `stado dns` reads the zone with
`namecheap.domains.dns.getHosts`, merges one record, and writes the whole zone
back, so the merge lives in Stado and every product's DNS goes through one
command. The credential is the Skarbiec item `namecheap_auto` (`api_user`,
`api_key`, `username`, `client_ip`).

**`stado web build`** — the build a Node web product's release runs. Twenty-four
landing sites and ten applications do not need thirty-four build scripts, so the
recipe in `.wisent-release.json` calls one Stado command and the manifest stays
declarative.

**`stado web edge`** — the edge host: provision it, declare it, install the
Stado-managed reverse proxy on it, and reconcile the set of hostnames it
terminates.

## The `.wisent-release.json` shape for a web product

A web product declares one platform whose key is `web`. `runner_platform`
names the host that builds it. The quality gate and the build both call
`stado web`, and the stage map names the one tarball the unit is installed
from.

```json
{
  "schema_version": 1,
  "product": "preferences-landing",
  "releases": true,
  "version_source": { "kind": "json", "path": "package.json", "pointer": "/version" },
  "platforms": {
    "web": {
      "runner_platform": "darwin-arm64",
      "quality": [
        { "name": "web-quality", "argv": ["stado", "web", "quality"] }
      ],
      "build": { "argv": ["stado", "web", "build"] },
      "stage": {
        "dist/preferences-landing-web.tar.gz": "preferences-landing-web.tar.gz",
        "dist/preferences-landing-web.tar.gz.sha256": "preferences-landing-web.tar.gz.sha256"
      }
    }
  },
  "promotion": { "channels": ["candidate", "stable"], "reconcile": false }
}
```

`stado web quality` reads the checkout the release worker prepared
(`WISENT_SOURCE_DIR`), installs the locked dependency tree, and runs the
product's own `typecheck` script when it declares one. `stado web build` runs
the product's `build` script and stages a tarball holding `.next`, `public`,
`package.json`, the production `node_modules`, and a generated launcher that
executes the product's `start` script on the port the unit passes it. A product
whose build needs a credential names it in the platform's `secret_env` as
`VAR: "item#field"`.

A build-time value that is not a credential goes in the platform's `env`
instead, as a literal:

```json
"env": { "NEXT_PUBLIC_SITE_URL": "https://content.wisent.ai" }
```

`stado web build` exports both maps into the install and the build. The split
is not cosmetic. `secret_env` names a Skarbiec item and field, so a public
constant kept there becomes a credential the fleet grants, syncs and audits
for nothing, and its value stops being visible in the diff that changed it.
`echo-web`'s build refuses to run without `NEXT_PUBLIC_SITE_URL`, and that URL
is on the public internet. One variable in both maps is refused: it would have
two answers.

### A static site is a web product too

Six of these sites are directories of files — `byk-landing`,
`handtohuman-landing`, `kronika-landing` and `transcript-lake-landing` are
`index.html` and a stylesheet at the top of the repository, `jeden`'s site is
its `web` directory, and `tama-landing` builds one into `dist/`. Vercel served
them by copying the directory, and they are hosted here the same way rather
than being rewritten as Node servers.

**One rule decides which a product is: a product that declares a `start`
script is a server, and a product that does not is a static site.** Nothing
else is consulted — not `next.config.*`, not a dependency on `next`, not which
directories exist. `start` is what the launcher runs, so "is there a server to
start" and "what does this product declare" have to be one question. A product
with a `build` and no `start` is a site whose build writes its files; a product
with neither is a site whose files are committed, and it needs no
`package.json` at all.

**The site root is what `--root` names in the build argv, and the checkout
root when nothing names one.** There is deliberately no probe order over
`public`, `site`, `dist` and `.`: a probe order is a guess, and a build that
guesses stages a directory nobody declared.

```json
"platforms": {
  "web": {
    "runner_platform": "darwin-arm64",
    "quality": [{ "name": "web-quality", "argv": ["stado", "web", "quality", "--root", "dist"] }],
    "build": { "argv": ["stado", "web", "build", "--root", "dist"] },
    "stage": {
      "dist/tama-landing-web.tar.gz": "tama-landing-web.tar.gz",
      "dist/tama-landing-web.tar.gz.sha256": "tama-landing-web.tar.gz.sha256",
      "dist/SOURCE_REVISION": "SOURCE_REVISION"
    }
  }
}
```

The artifact holds the site under one fixed directory, `site/`, plus the same
`bin/start-web` launcher and a `bin/serve-static.mjs` server generated into the
tarball. That server uses only `node:http`, `node:fs` and `node:path`: a global
`serve` would be a dependency of the host rather than of the release, and
`npx serve` would fetch from the npm registry the first time a unit started —
a network call in the run path, resolving a version nobody pinned, on a host
whose whole point is that it runs the bytes someone published. It resolves a
request as an exact file, then `<path>.html`, then `<path>/index.html`, which
is the `cleanUrls` behaviour these sites were served with, answers `404.html`
when the product ships one, refuses anything but `GET` and `HEAD`, and confines
every resolved path to the site root.

The launcher is the same script with one line different, so the unit's
contract does not change: the install root resolved from `$0`, `PORT` passed by
the unit and refused when unset, `WEB_ENV_FILE` sourced with assignments
exported, and `127.0.0.1` only. **`stado web deploy` is unchanged** — it
installs a tarball and runs `bin/start-web`, and it cannot tell the two kinds
apart. When the site root is the checkout root, `.git`, `.github`, `.vercel`,
`.next`, `node_modules`, `release/` and any `.env*` file are left out: they are
not part of the site, and one of them is a credential.

### A web platform beside a binary platform

A product can host a site and ship a binary from one repository. `jeden` is a
Rust CLI whose documentation site is its `web` directory, and its
`.wisent-release.json` declares a `runtime` contract — the binary, its
launcher and the schema versions the rollout state machine reads.

That contract is declared once for the product, and it applies to the
platforms it describes. A platform whose stage map contains both the
runtime's `binary` and its `launcher` ships the runtime and is held to the
contract; a platform whose stage map contains neither ships something else,
like a site tarball, and the contract says nothing about it. A platform with
exactly one of the two is still refused, because that is the half-staged case
the check exists for: a rollout would install something the host cannot
start. A `runtime` that no platform stages at all is refused too — it is a
declaration nothing checks against the world, and the first rollout would be
what discovered it.

`stado release submit` reads the same rule when it publishes: a platform that
ships no runtime publishes no binary path, no launcher and no schema
versions, rather than inheriting the product's and claiming a binary that
exists in none of its own bytes. Rollout is unaffected either way — a rollout
target names the platform it rolls out, in the product's rollout policy, so a
web platform is only ever rolled out if someone declared it there.

## The hostname and DNS model

One web product owns one hostname. The hostname's zone is whatever the
registrar says it is, and Stado does not guess: `stado web declare` records the
hostname, and `stado web route` resolves its zone, writes the record through
the edge the product declares, and reports what the record became.

For the Stado edge, `stado web route` does three things in an order that is
forced rather than chosen.

First the hostname is reconciled into the edge proxy's configuration, which is
rendered whole from the product declarations, so a second `route` for the same
product changes nothing. Then the A record is written to the edge's public
address by `stado dns set` — a whole-zone read, a merge of one name, a
whole-zone write. Only then can the certificate exist: Let's Encrypt delivers
its HTTP-01 or TLS-ALPN-01 challenge to whatever the hostname resolves to, so
Caddy cannot obtain a certificate for a name that does not yet point at it.

That is why the third step is the one that decides the verdict. The command
polls `https://<hostname>` until it answers over TLS with a 2xx and no
`x-vercel-id` header, and reports how long that took. Between the record
moving and the certificate existing there is a real window in which the name
resolves to an edge that cannot complete a handshake; success is never
reported on anything less than a completed TLS request, and a hostname that
still answers from Vercel is reported as unpublished with the `server` and
`x-vercel-id` values actually observed.

The site block existing before the record moves is what keeps that window
short: the first request to arrive after the cutover finds a proxy that knows
the name and can begin an issuance, rather than one that has never heard of it.

For a zone at Cloudflare the record is written by the Cloudflare API as a
proxied `CNAME` to the tunnel, and the tunnel's ingress rule is written in the
same command.

Nothing removes a Vercel project. A hostname stops being served by Vercel when
its DNS record stops pointing there, and that record is the last step of
`stado web route`.

### A redirect is a product with no unit

Five of the fleet's Vercel projects are one rewrite each: `aiwisent.com`,
`getwisent.com`, `trywisent.com`, `wisentai.com` and `wisentplatform.com` all
answer `https://wisent-app.com/:path*` and nothing else. Their repositories
carry a `vercel.json` with a single redirect, no framework and no build. There
is no application to host, and expressing one as a web product with a port and
a consumer would mean declaring a unit nobody runs.

So a product may declare a hostname and a target instead:

```console
stado web declare aiwisent-com \
  --hostname aiwisent.com \
  --redirect-to https://wisent-app.com
stado web route aiwisent-com
```

`--host`, `--port`, `--consumer`, `--readyz`, `--env`, `--secret` and
`--database` are refused beside `--redirect-to`, and the configuration parser
refuses the same combination: a declaration carrying both says two different
things about what the product is. The edge renders a `redir` block rather than
a `reverse_proxy`, and the rest of the path is unchanged — the hostname is
reconciled into the edge, the record is written by `stado dns`, and the
certificate is ordered for it like any other:

```
aiwisent.com {
	redir https://wisent-app.com{uri} 308
}
```

`{uri}` carries the path and the query across, which is exactly what the
Vercel rewrite's `/:path*` did. The status is 308 rather than 301 because 308
preserves the method and the body: a `POST` to the old hostname arrives at the
new one as a `POST`, where 301 lets a client turn it into a `GET` and a form
submission silently becomes a page load. Permanent either way — these
hostnames are not coming back.

The target must be `https://` with a public host name, no query, no fragment
and no trailing slash. A redirect Stado publishes on its own edge must not
send a browser to a hostname it holds no certificate for, a query would
collide with the appended `{uri}`, and a trailing slash would make every
redirected path a double slash. A path prefix is allowed:
`--redirect-to https://wisent-app.com/pricing`.

`stado web deploy` refuses a redirect and says which command publishes it;
`stado web status` reports it as `kind: redirect` with its target and whether
its hostname resolves to the edge, rather than asking a host about a unit that
was never deployable; `stado web remove` retracts the hostname and reports
`no-unit`.

### A hostname in front of an existing service

`brama.wisent.com` is not a web product. Brama is a Rust binary with its own
managed unit on the mini, answering `127.0.0.1:18081`, and it is already
published over public HTTPS — by a Vercel project whose whole content is a
catch-all rewrite to `https://charless-mac-mini.tail6443b3.ts.net/:path*`.
Vercel contributes exactly one thing there: a certificate for a `wisent.com`
name. That is the thing this edge exists to do.

So a product may declare a hostname in front of a service the registry
already runs:

```console
stado web declare brama --hostname brama.wisent.com --upstream-service brama
stado web route brama
```

No `--host` and no `--port`. The service directory already records which host
the service is active on and which address it answers, and a copy of either in
this declaration would point at the old host the day the service moved — so
the upstream is resolved out of the directory on every render, not snapshotted
when the command was typed. `--host`, `--port`, `--consumer`, `--readyz`,
`--env`, `--secret` and `--database` are refused beside it, and so is
`--redirect-to`: a hostname either answers with a redirect or forwards to a
service, never both.

The rendered site block is an ordinary `reverse_proxy` at the service's own
address on its active host, reached over the tailnet:

```
brama.wisent.com {
	reverse_proxy http://charless-mac-mini:18081
}
```

**Nothing here deploys, installs or stops that service.** `stado web deploy`
refuses and names `stado service status` and `stado service deploy` as what
owns the unit; `stado web status` reports `kind: upstream-service` with the
service's host, state and last beacon time read out of the same fleet-wide
join every other row uses; `stado web remove` retracts the hostname and
reports `no-unit`, leaving the service running. A `stado web` command
stopping Brama because someone removed a hostname in front of it would be
this plane reaching into a service it does not own.

### A path under another product's hostname

`brama.wisent.com` is two things. Its catch-all belongs to Brama, published
by the declaration above; `/docs` is 79 documentation pages, versioned with
Brama in `vercel-ingress/docs/`, which the retiring Vercel ingress served
through 57 `/docs*` rewrites onto static HTML. Brama does not serve them
itself, so the hostname needs two answers.

A mount is an ordinary unit product — built, released and deployed like any
other — whose hostname belongs to a different declaration:

```console
stado web declare brama-docs \
  --hostname brama.wisent.com \
  --path-prefix /docs \
  --host charless-mac-mini \
  --port 3220 \
  --consumer brama-docs-web
stado release submit brama --channel stable
stado web deploy brama-docs
stado web route brama-docs
```

The edge renders it inside the owner's site block, ahead of whatever answers
the rest:

```
brama.wisent.com {
	handle_path /docs* {
		reverse_proxy http://charless-mac-mini:3220
	}
	reverse_proxy http://charless-mac-mini:18081
}
```

`handle_path`, not `handle`: it strips the matched prefix before proxying, so
`/docs/core` arrives at the unit as `/core` — which is the unit's own path,
because the unit is a site whose pages start at `/`. `handle` would forward
`/docs/core` unchanged and every page would answer 404. The matcher is
`<prefix>*` so the prefix itself matches too, and the order inside the block
is the semantics: Caddy takes the first matching route, so a catch-all
rendered before the mount would answer `/docs` itself and the mount would
never be reached. Several mounts on one hostname are rendered longest prefix
first, so `/docs/api` cannot be swallowed by a `/docs` beside it.

The prefix is absolute with no trailing slash — the matcher is `<prefix>*`,
and `/docs/` would stop `/docs` itself from matching — and carries no
wildcard, brace or whitespace of its own, because those would rewrite the
matcher into something nobody declared. A mount may not also declare
`--redirect-to` or `--upstream-service`: both of those answer a whole
hostname.

What the configuration plane refuses: a mount whose hostname no declaration
owns, because a mount is rendered inside its owner's block and one without an
owner is a block with nowhere to go — the hostname would get no certificate
at all; two mounts at the same prefix on one hostname, because that renders
two `handle_path` blocks for one path and the first would silently win; and
a second declaration owning a hostname that is already owned, which was
already true.

`stado web route` for a mount **writes no DNS record**. The hostname's A
record belongs to the declaration that owns it and already points at this
edge; a second writer of one name is how a mount's removal would look like it
should take the record with it. It verifies `https://<hostname><prefix>/`
instead of the owner's readiness path, because the owner's `readyz` is a path
on the owner's application and says nothing about whether `/docs` reaches
this unit. `stado web remove` retracts the mount only: one `handle_path`
block comes out of the owner's site block, the record stays, and every other
mount under that hostname stays.

## The command sequence

Stand up the edge once for the whole fleet:

```console
stado web edge provision --name wisent-edge --region westus2 \
  --size Standard_B2pts_v2 --contact ops@wisent.com
stado web edge status
stado web edge hostnames
```

`provision` goes through Stado's own Azure provider
(`crate::providers::azure`) and the same subscription, resource group, subnet
and SSH key every other Stado-provisioned VM uses, so the edge is cloud
capacity the fleet accounts for like any other — not a machine that exists
outside the plane. It creates a public address, an edge-only security group
opening 80 and 443, a NIC and the VM, in that order, and unwinds them in
reverse if any step fails, naming anything Azure would not release.

`stado web edge remove` is the one command that undoes it. It deletes the VM,
the NIC, the security group and the public address in reverse creation order,
waiting for each, because Azure refuses to release an address while an
interface still references it and an orphaned address is billed while
belonging to nothing. It refuses while any product still names the `stado`
edge — those hostnames' A records point at the address it is about to hand
back — and `--orphan-hostnames` is how an operator overrides that.
`--keep-resources` forgets the declaration and touches nothing on Azure, which
is the correct removal for an edge recorded with `stado web edge declare`
rather than created here.

```console
stado web edge remove
```

Declare a product:

```console
stado web declare preferences-landing \
  --host charless-mac-mini \
  --port 3210 \
  --hostname preferences.wisent.com \
  --consumer preferences-landing-web
```

Release it from its checkout, which builds it on the fleet and publishes the
tarball into the channel:

```console
stado release submit preferences-landing --channel stable
```

Install and start the unit, deliver its environment, and verify it:

```console
stado web deploy preferences-landing
stado web status preferences-landing --json
```

`deploy` installs the released tarball as a managed unit, mints the unit's
Skarbiec consumer grant, and for a product that declares a database resolves
the credential item for that consumer through `stado database resolve` and
delivers the named field with `stado service secret-sync`. No value is read by
the operator and none appears in a command line.

Publish the hostname:

```console
stado web route preferences-landing
```

`route` reconciles the hostname into the edge, writes the record, and then
waits for `https://<hostname>` to answer over TLS, reporting the elapsed
seconds. It does not report success on a 2xx that still carries an
`x-vercel-id` header.

An application with a database and an operator token declares them when it is
declared:

```console
stado web declare preferences \
  --host charless-mac-mini \
  --port 3211 \
  --hostname app.preferences.wisent.com \
  --consumer preferences-web \
  --database preferences \
  --database-field pooler_url \
  --database-variable DATABASE_URL \
  --secret PREFERENCES_OPERATOR_TOKEN=preferences-operator-api#token \
  --env NEXT_PUBLIC_BASE_URL=https://app.preferences.wisent.com \
  --readyz /api/readyz
```

`stado web list` shows every declared product with its host, port, hostname and
unit. `stado web remove` stops the unit, forgets it, and removes the hostname's
record.
