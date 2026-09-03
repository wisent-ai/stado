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
consumer the declaration does not list (`stado-rs/src/cli/database.rs`,
`database_api` in the configuration). The value never leaves Skarbiec through
this path; the item name does.

## What the zones and the credentials actually are

Everything about the ingress follows from six measurements, all taken on
2026-09-02.

`dig +short NS wisent.com` (and the same for `wisent.ai` and `needher.ai`)
answers `dns1.registrar-servers.com` and `dns2.registrar-servers.com`. All
three zones are served by Namecheap. None of them is on Cloudflare.

`curl -sI https://preferences.wisent.com/` answers `200` with `server: Vercel`
and an `x-vercel-id` header, and the hostname resolves to `76.76.21.21`. The
same is true of `app.preferences.wisent.com` and of `brama.wisent.com`, which
fronts a service that runs on the fleet. Vercel is the TLS edge for
`*.wisent.com` today, including for surfaces the fleet already serves.

Skarbiec holds two Cloudflare items. `platform-admin-cloudflare` carries
`username` and `password` — a console login, not an API credential.
`platform-cloudflare-bobloo-tunnel` carries `account_id`, `token`, `tunnel_id`
and `tunnel_name`, and its `token` is a 180-character `cloudflared` tunnel
token: presented to the Cloudflare API as a bearer it is refused with code
6111, `Invalid format for Authorization header`. There is no Cloudflare API
token in the vault, and `stado cloudflare` requires one
(`--api-credential`, whose item must carry `account_id` and `api_token`).

`stado host exec ubuntu-server-rtx-pro-6000 -- tailscale netcheck` reports
`IPv4: yes, 24.23.232.108:56883`. `curl -s -4 https://api.ipify.org` from the
operator's laptop answers the same address. Every fleet host sits behind one
residential connection, and inbound TCP to 80 and 443 on that address times
out. No fleet host has a public IPv4 address.

`stado host exec charless-mac-mini -- tailscale funnel status` reports Funnel
on for `https://charless-mac-mini.tail6443b3.ts.net` on ports 443, 8443 and
10000, each forwarding to a loopback origin. That is the fleet's only public
entrance, and Tailscale states its limit plainly: Funnel can only use DNS names
in the tailnet's own domain. A request whose SNI is `preferences.wisent.com`
has nowhere to go in that path, whatever DNS says.

`stado host exec charless-mac-mini -- lsof -nP -iTCP -sTCP:LISTEN` shows
`cloudflared` already running on the mini on `127.0.0.1:20241`, `node` serving
on three ports, and `caddy` with its admin API on `127.0.0.1:2019`.

## Why the ingress is a Cloudflare Tunnel

A public `https://<hostname>` on the operator's own domain needs two things at
the same place: a route the public internet can reach, and a certificate for
that exact hostname. The fleet has the first only through Tailscale Funnel,
which cannot supply the second for anything outside `*.ts.net`. So the
certificate has to be issued and presented by something in front of the fleet.

A Cloudflare Tunnel is the mechanism that fits, and it is the only one that
costs nothing and adds no machine. `cloudflared` on a fleet host dials out to
Cloudflare, so no inbound port and no public address are needed; Cloudflare
terminates TLS for the hostname with a certificate it issues and manages; and
the hostname's DNS record is a `CNAME` to `<tunnel_id>.cfargotunnel.com`, which
only Cloudflare can resolve to the tunnel. Stado already speaks exactly this
protocol in `stado cloudflare route-tunnel`: it reads the tunnel's ingress
configuration, adds the hostname's rule, writes the configuration back, and
creates the proxied `CNAME`.

Cloudflare issues that edge certificate only for a hostname in a zone it
serves. For `preferences.wisent.com` that means the `wisent.com` zone moves to
Cloudflare's nameservers, with every record it holds today — including the
Google Workspace MX records — re-created there first. Delegating only the
`preferences.wisent.com` subtree as its own Cloudflare zone would avoid
touching the apex, but subdomain zones and CNAME-only ("partial") setup are
Business-plan features, so that variant costs money rather than a nameserver
change.

**This is the operator's decision, and it is the one thing on this page that is
not Stado's to take.** Until it is taken, `stado web route --edge cloudflare`
refuses with the reason rather than half-publishing a hostname, and
`stado web route --edge funnel` publishes the product on the fleet's own public
entrance, which needs no zone change and proves everything except the custom
hostname's certificate.

## What was added

**`stado web`** — the product-level capability. It owns the shape of a web
product: which release artifact it runs, on which host and port, under which
Skarbiec consumer, with which environment, behind which hostname.

**`stado dns`** — the registrar. Namecheap's `setHosts` call replaces a whole
zone, so a record cannot be changed without re-sending every other record in
it; that is why `wisent.com`'s records were written by a script inside a
product repository. `stado dns` reads the zone with
`namecheap.domains.dns.getHosts`, merges one record, and writes the whole zone
back, so the merge lives in Stado and every product's DNS goes through the same
command. The Namecheap credential is the Skarbiec item `namecheap_auto`
(`api_user`, `api_key`, `username`, `client_ip`).

**`stado web build`** — the build a Node web product's release runs. Twenty-five
landing sites and five applications do not need twenty-five build scripts, so
the recipe in `.wisent-release.json` calls one Stado command and the manifest
stays declarative.

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

## The hostname and DNS model

One web product owns one hostname. The hostname's zone is whatever the
registrar says it is, and Stado does not guess: `stado web declare` records the
hostname, and `stado web route` resolves its zone, writes the record through
the edge the operator names, and reports what the record became.

For a zone at Namecheap the record is written by `stado dns set`, which is a
whole-zone read, a merge of one name, and a whole-zone write. For a zone at
Cloudflare the record is written by the Cloudflare API as a proxied `CNAME` to
the tunnel, and the tunnel's ingress rule is written in the same command, so a
hostname never resolves to a tunnel that does not carry it.

Nothing removes a Vercel project. A hostname stops being served by Vercel when
its DNS record stops pointing there, and that record is the last step of
`stado web route`.

## The command sequence

Declare the product, once:

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
stado web route preferences-landing --edge cloudflare
```

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
