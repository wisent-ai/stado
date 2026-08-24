# Walkthrough: release end-to-end

How does a committed source tree become the exact bytes every fleet host
runs, and how do you take it back? This page walks one release from source to
fleet and back as commands and their readings. The strict manifest and
catalog contract lives in [release](release.md); what a release *is* lives in
[primitives/release](primitives/release.md); the operational entry points
live in [operations](operations.md).

## Catalog first

Stado owns product release policy independently of repository hosting:

```bash
stado release catalog sync --catalog /path/to/release-catalog.json
stado release catalog audit
```

Sync imports the fleet's reviewed central catalog — including explicit
`releases:false` manifests — refuses missing or duplicate product names, and
CAS-updates `stado://system/release-catalog/<product>.json`; `--root ROOT`
bootstraps a catalog from local registered checkouts. Audit reads only Stado
catalog objects and refuses malformed, duplicate, or silent catalogs; it does
not enumerate a Git forge or require forge tokens.

## Submit

```bash
stado release submit --source /path/to/product --version <exact-version> \
  --channel candidate
```

Submit requires a clean committed tree and never contacts a Git remote. It
verifies `--version` against the product's checked-in version source,
archives the exact committed tree, publishes the create-only source object at
`stado://sources/<product>/<sha256>/source.tar.gz`, records source and
manifest identity in the catalog, and creates one provider-neutral queue job
pinned to a registry builder whose `release_platform` matches the recipe.

Run identity is derived from product, version, channel, source digest, and
manifest digest, so repeating the same submit resumes the run instead of
starting another. The durable run record at
`stado://<queue-namespace>/runs/release-pipeline/<id>/run.json` shows job
IDs, output coordinates, delivery state, and failure. A terminal successful
platform output is read from JobStorage and published, never rebuilt.

## Qualification, signing, publication

Builders receive no repository coordinate or repository token: they
materialize only the exact source URI and declared immutable inputs, run the
manifest's quality argv in order and one build argv, and write
`status/<job>/output/{receipt.json,release.tar.gz}` through the queue's
canonical job-output collection.

Publication under the immutable coordinate
`stado://releases/<product>/<version>/<platform>/` is ordered:

```text
release.tar.gz -> qualification.json -> release.sig -> release.json
```

The signed manifest (`release.json`) is written last and is the commit
marker: required delivery jobs run only after it exists and consume its exact
URI and digest. The signing key stays in Skarbiec — only the item name
(`stado-release-signing`) and the trusted key ID are configuration, never a
secret value in a manifest or command line. Optional mirrors (a Git forge
among them) do not gate canonical success.

## Promote

```bash
stado release promote <product> <version>
```

Promotion changes references, never bytes: it re-fetches every platform,
verifies the exact bytes, signature, and passed qualification, then
compare-and-swaps one desired `registry.release_control` generation. It never
rebuilds.

## Hosts converge

The per-host declaration is `targets[].managed_versions` — the exact
semantic version each stado-managed binary must run on that host, written by
the operator:

```bash
stado host declare-version <target> --binary <name> --version <X.Y.Z>
stado service converge <target>
```

`converge` compares each declared version against what the host actually
runs. Verdicts are `in-sync`, `drifted`, and `unknown` for a binary whose
installed version could not be read; `unknown` is never folded into either of
the other two, so an uninstalled reporting helper cannot masquerade as drift.
Reporting exits non-zero on `drifted` alone; `--apply` delivers the declared
version of every drifted binary through `stado host release`, re-reads the
installed versions afterwards, and exits non-zero unless every binary in
scope is confirmed `in-sync`. `converge` never writes the registry: the
declared version is the operator's statement of intent, and a converge that
edited the document to match the host would turn a drift report into a
rubber stamp.

Runtime products on registry hosts also converge without an operator:
`stado release agent` reconciles desired releases on its exact registry
target from the `release_control` desired generation.

## Verify the rollout

```bash
stado release status <product>
```

`status` joins desired state with each host's observed rollout state and the
host's own software report, and exits non-zero for a host that has never
reported, reports stale, or runs unmanaged bytes — silence is a failure,
never a pass. The rollout is done when `deployment.json` exists: it is
written only after every declared target reports the promoted version,
artifact digest, and manifest digest exactly.

## Back out

```bash
stado release rollback <product>
```

Rollback atomically restores the previous desired release — the exact
previously recorded coordinate, never a rebuild in place.

If one host keeps refusing a version it was already given, check quarantine.
The release agent quarantines a digest that failed to become ready and never
retries it on its own — correct, since a candidate that dies in ninety
seconds must not respawn in a loop:

```bash
stado release quarantine list <product> --target <target>
stado release quarantine clear <product> --target <target> \
  --digest <digest> --reason "<why this digest gets another chance>"
```

`clear` starts nothing, restarts nothing, and kills nothing: it removes one
map entry, and the agent's next tick finds the desired digest no longer
quarantined and rolls it out on its own — the same path it would have taken
had the digest never failed. `--target` and `--reason` are never inferred or
defaulted; the reason is recorded in the audit trail beside the host's
rollout state.

Symptom-first triage for a release that did not land lives in the
[runbook](runbook.md); flag-by-flag detail lives in [cli](cli.md).
