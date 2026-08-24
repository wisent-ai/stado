# Quick start

How do you go from no Stado state to one completed job? This page is the one
happy path: install an exact release, create the minimal local configuration,
start the local control plane, submit one job, and read its result. Local
mode needs no cloud account, provider CLI, GPU, or Wisent production
credential. Everything else — the other enrollment methods, failure guidance,
attaching more machines — lives in [onboarding](onboarding.md),
[add-your-machine](add-your-machine.md), and [jobs](jobs.md).

## Install an exact release

`install-stado.sh` at the repository root installs one exact immutable
archive after verifying its canonical manifest; it never resolves a mutable
release. Set the release identity, run it, and expose its binary directory:

```bash
export STADO_API_URL=<your-control-origin>
export STADO_RELEASE_VERSION=<exact-version>
export STADO_RELEASE_PLATFORM=<exact-platform>
./install-stado.sh
export PATH="$HOME/.stado/bin:$PATH"
```

The script requires an HTTPS `STADO_API_URL`, fetches the release manifest
and archive from `/api/release/object`, verifies the manifest's shape and the
archive's SHA-256 digest, and refuses an archive with unexpected, duplicate,
or missing members before replacing any binary. Binaries land in
`$HOME/.stado/bin` (override with `STADO_BIN_DIR`), and success prints
`installed Stado <version> for <platform> in <directory>`.

## Create the minimal configuration

`STADO_CONFIG` selects the operator-owned deployment profile; without it,
Stado uses `~/.stado/config.json`, which the next command creates:

```bash
stado config init
stado config validate
```

`config init` prints the path to the new file and creates only a
schema-versioned local queue profile: local compute, local primary and backup
stores, one deployment identity, and a loopback dashboard — no Wisent service
routes, cloud locators, or credentials. It never overwrites an existing file.
`config validate` prints `config ok (<path>)`. The keys themselves are
documented in [configuration](configuration.md).

## Start the local control plane

```bash
stado local-control-plane
```

This runs the device-local API listener, scheduler, and worker. Leave it
running; it binds to loopback (`127.0.0.1:8765`) by default.

## Submit one job

From another terminal using the same config:

```bash
stado submit "printf 'hello from Stado\n'"
```

`submit` puts the job on the queue and prints a `Job ID`. Use that ID below.

## Watch it run

```bash
stado status <job-id>
```

`status` shows the job's state; it progresses from queued or running to
completed. The argument filters by job id (8 hex chars) or batch id
substring.

## Read the result

```bash
stado results <job-id> ./stado-result
```

`results` downloads the job's output into the named directory. The command
output contains `hello from Stado`, and the result manifest records the
artifact size and SHA-256. A failed job remains inspectable and may still
publish logs and partial artifacts.

That is the whole path. To onboard more machines or operate a real fleet,
continue with [onboarding](onboarding.md); to attach your own computer to a
fleet someone else runs, read [add-your-machine](add-your-machine.md); for
the full job contract — constraints, artifacts, secrets, verification — read
[jobs](jobs.md).
