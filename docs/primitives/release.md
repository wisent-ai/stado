# Release

What exactly is a release in Stado, and what makes it safe to put on a fleet
host? A release is an immutable, signed set of objects whose identity is
verified at every step from publication to the running process.

## What it is

A release is published under an immutable coordinate,
`stado://releases/<product>/<version>/<platform>/`, in one canonical order:

```text
release.tar.gz -> qualification.json -> release.sig -> release.json
```

The signed manifest (`release.json`) is written last and is the commit marker:
required delivery jobs run only after it exists and consume its exact URI and
digest ([operations](../operations.md)). The manifest binds product, SemVer,
platform, source revision, archive digest and size, binary and launcher paths,
config and state schemas, minimum Stado version, rollback compatibility,
qualification evidence, builder and key id
([architecture](../architecture.md)). Promotion changes references, never
bytes: `stado release promote` re-fetches every platform, verifies the exact
bytes, signature and passed qualification, then compare-and-swaps one desired
registry generation. It never rebuilds ([cli](../cli.md)).

Reads under `stado://releases/` are public and bearer-free through the one
durable origin, `https://stado.wisent.com/api/release/object`. Publication is
authenticated and create-only. The signing key stays in Skarbiec: only the
item name (`stado-release-signing`) and the trusted key id are configuration
([operations](../operations.md)); the consumer that reads it is a dedicated
[grant](grant.md).

## Who declares it

`stado release submit` starts from a clean committed tree, verifies the
explicit `--version` against the product's checked-in version source, and
never contacts a Git remote. Run identity is derived from product, version,
channel, source digest, and manifest digest, so repeating the same submit
resumes the run; a published platform is verified, never rebuilt
([release](../release.md)).

The per-host declaration is `targets[].managed_versions` in the
[registry](registry.md): the exact semantic version each stado-managed binary
is required to be at on that host. A target that omits it declares nothing and
is reported `undeclared`, never as agreeing
([architecture](../architecture.md)). Operators write it with
`stado host declare-version TARGET --binary NAME --version X.Y.Z`.

## Who observes it

The host re-verifies every manifest binding before extraction and rejects
links, traversal, excessive entry counts or expanded size
([architecture](../architecture.md)).

`stado service converge` is the reconciliation of `managed_versions`: per
declared binary it compares the declared version against what the host
actually runs ([cli](../cli.md)):

| Verdict | Meaning |
|---|---|
| `in-sync` | The host runs exactly the declared version. |
| `host-behind` | The host runs strictly older; `--apply` delivers the declared version and re-reads afterwards. |
| `host-ahead` | The declaration is stale; delivery would downgrade a live host, so `--apply` refuses and names the `declare-version` command that fixes the document. |
| `unknown` | Nothing usable came back; never silently treated as `in-sync`. |

`converge` never writes the registry: the declared version is the operator's
statement of intent, and a converge that edited the document to match the host
would turn a drift report into a rubber stamp. It also reports which artefact
the live process is actually running (`running_binary`,
`binary_matches_process`), because an installed version says nothing about a
process that started before it ([cli](../cli.md)).

`stado release status` joins desired state with each host's observed rollout
state and the host's own software report, and exits non-zero for a host that
has never reported, reports stale, or runs unmanaged bytes — silence is a
failure, never a pass ([cli](../cli.md)).

## Where it lives

| Coordinate | Contents |
|---|---|
| `stado://sources/<product>/<sha256>/source.tar.gz` | Create-only deterministic source archive with commit and manifest identity. |
| `stado://releases/<product>/<version>/<platform>/` | Archive, qualification receipt, signature, signed manifest — immutable, in that order. |
| `stado://<queue-namespace>/runs/release-pipeline/<id>/run.json` | Durable run record: job IDs, output coordinates, delivery state, failure. |
| `registry.release_control` | Desired generation, moved only by compare-and-swap. |
| `deployment.json` | Written only after every declared target reports the promoted version, artifact digest, and manifest digest exactly. |

## Commands

```bash
stado release submit --source /path/to/product --version 1.4.2 --channel candidate
stado release catalog sync --root /path/to/registered-checkouts
stado release catalog audit
stado host declare-version control-host --binary stado --version 0.6.0
stado service converge control-host
stado service converge control-host stado --apply
```

Flag-by-flag detail lives in [cli](../cli.md).

## Not to be confused with

- **A build.** Builds publish artifacts and record versions; they never write
  `release_control.products[...]` desired state. Promoting a signed release is
  a separate, deliberate step ([cli](../cli.md)).
- **A git commit or channel.** `managed_versions` values are exact semantic
  versions — a channel, alias or range cannot be compared for equality, and
  equality is the whole of the question ([cli](../cli.md)).
- **A mirror.** Optional deliveries (a Git forge among them) may mirror
  completed releases, but their availability cannot change source,
  qualification, signing, desired state, or observed rollout truth
  ([operations](../operations.md)).
