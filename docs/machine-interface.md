# Machine Interface

You are integrating a program, not an operator: how does it submit a job,
watch it, cancel it, and collect its artifacts without parsing text meant for
humans? `stado machine` is that surface. Its own help calls it the "Stable
JSON machine interface": every command prints exactly one JSON envelope line
on stdout, and stderr stays clean for automation. Flag-by-flag detail lives in
the [cli](cli.md) reference; job semantics live in [jobs](jobs.md) and
[primitives/job](primitives/job.md).

## The envelope

Every invocation ends the same way, regardless of subcommand:

- Success: `{"schema_version":1,"ok":true,"result":...}` and exit 0.
- Failure: `{"schema_version":1,"ok":false,"error":{"code":...,"message":...,"retryable":...}}`
  and exit 1.

The envelope is serialized as canonical JSON — sorted keys, compact
separators, non-ASCII preserved. A program therefore parses the single stdout
line, branches on `ok`, and on failure branches on `error.code` plus the
`retryable` flag; it never needs to scrape stderr or interpret prose.

Error codes are part of the contract. The set includes `INVALID_REQUEST`,
`IDEMPOTENCY_CONFLICT`, `NOT_FOUND`, `INVALID_CURSOR`, `NOT_TERMINAL`,
`ARTIFACT_SECURITY`, `NO_ARTIFACTS`, and `SERVICE_DIRECTORY_STALE`.
Unexpected storage, IO, or JSON failures map to code `INTERNAL` with
`retryable` false.

## The five commands

| Command | The question it answers |
|---|---|
| `stado machine submit --request-file FILE` | Run this request exactly once, even if I retry this call. |
| `stado machine status JOB_ID` | What state is this one job in, read directly by ID? |
| `stado machine logs JOB_ID` | Give me the next page of the canonical command log, from a byte cursor. |
| `stado machine cancel JOB_ID` | Stop this job, durably, and tell me the same thing if I ask twice. |
| `stado machine artifacts --output-dir DIR JOB_ID` | Download this terminal job's canonical artifacts, verified. |

## Submit is idempotent by construction

`submit` takes a JSON request file. The request must carry a
`client_request_id` (letters, digits, `.`, `_`, `-`; 1–128 characters,
starting alphanumeric); the accepted fields are `client_request_id`,
`command`, `provider`, `gpu_type`, `pinned_host`, `vram_gb`,
`max_cost_per_hour_usd`, `pin_to_provider`, `priority`, `repo`, `repo_ref`,
`repo_workdir`, `repo_extras`, `pre_command`, `apt_packages`, `output_uri`,
`verify_command`, `exclusive`, `source_archive_path`, `input_objects`, and
`secret_env`.

```bash
stado machine submit --request-file request.json
```

The request is validated, then reserved under
`machine_requests/<client_request_id>.json` together with the SHA-256 digest
of the request. Retrying the exact same request replays the stored result;
sending a different request under the same `client_request_id` is refused
with `IDEMPOTENCY_CONFLICT`. A `source_archive_path` upload is bounded: at
most 512 MiB of archive, 2 GiB extracted, and 100,000 members.

## Status and logs

`status` looks the job up directly by ID across the canonical state prefixes
(`queue`, `running`, `completed`, `uploaded`, `failed`, `cancelled`) and
returns it under `result.job`; an ID found nowhere is `NOT_FOUND`.

`logs` pages over the canonical command log by byte offset:

```bash
stado machine logs --cursor 0 --limit 65536 "$JOB_ID"
```

The result carries `job_id`, `cursor`, `next_cursor`, `eof`, and `text`.
`cursor` is a byte offset into the log, `next_cursor` is where the next page
starts, and `eof` is true when the page reached the current end of the log —
so a poller carries `next_cursor` forward and never re-reads bytes it already
has. A negative cursor, non-positive limit, or a cursor beyond the end of the
log is `INVALID_CURSOR`.

## Cancel is durable and idempotent

```bash
stado machine cancel "$JOB_ID"
```

The cancellation marker `cancellations/<job_id>.json` is written first, so
the coordinator reaps the job even if this call dies mid-transition. The
provider instance is resolved through the recorded provider lease as well as
the job document, so a VM whose reference only ever reached the lease is
deleted too instead of billing forever. Cancelling a job that is already
terminal returns the job as-is — asking twice is safe.

## Artifacts are verified on the way down

```bash
stado machine artifacts --output-dir ./out "$JOB_ID"
```

`artifacts` downloads the canonical `status/<id>/output/` blobs of a terminal
job; a non-terminal job is refused with `NOT_TERMINAL`. Every blob is hashed
while streaming to disk and reported with its size and SHA-256. Output-path
and storage-path symlink and escape rules are enforced; a violation is
refused with `ARTIFACT_SECURITY` rather than written.

## Stability

The envelope is versioned: `schema_version` is `1`, and the error codes above
are stable identifiers a program may branch on. Not yet documented: a
compatibility policy for what a future `schema_version` change would preserve.
