# Wisent app backend production recovery

Last updated: 2026-08-28T17:09:54Z

This record is the operator-readable source of truth for restoring the production
Wisent app backend on `charless-mac-mini`. It contains no credentials or token
values. Update the timestamp, current state, blocker, timeline, and evidence rows
whenever an observed state changes.

## Acceptance criteria

Recovery is complete only when all of these observations pass:

- `com.wisent.compute.service.wisent-backend-api` is loaded and remains healthy,
  with `127.0.0.1:8000` listening for more than two minutes.
- `https://bobloo.com/` returns HTTP 200 rather than 502.
- An authenticated `POST /api/chat/send` produces SSE containing actual assistant
  text and no embedded error payload.
- Product/service-recorded image-router status is healthy. A paid image generation
  is not required and must not be triggered merely as a health check.
- The backend uses its canonical content-addressed Stado release URI and digest.

## Current production state

**Not recovered.** At 2026-08-28T17:09:54Z the sanctioned
`stado host inventory charless-mac-mini --json` report did not contain a listener
on port 8000. Direct public checks of the app root and chat route each returned
HTTP 502. The mini reported Stado `0.7.29` while its declaration was `0.7.45`;
Skarbiec `0.2.8` matched its declaration. The Stado object listener on
`127.0.0.1:8765` and canonical Skarbiec listener on `127.0.0.1:8895` were present.

### Current single blocker

The managed owner-host Stado resolver still runs the pre-fix transport. Public
release-channel reads truncate and the client reports `error decoding response
body` at `cli.storage.get`. The durable source fix is commit
`baf6f0c837d1fdbe5d08e05aa448c3d403c850be` on PR
[#132](https://github.com/wisent-ai/stado/pull/132): it removes the contradictory
`ControlMaster=no` override and reuses one canonical SSH option construction.
Hosted qualification run
[33185279242](https://github.com/wisent-ai/stado/actions/runs/33185279242),
attempt 6, failed at 2026-08-28T17:05:32Z while reading the immutable public
baseline, before the fixed binary could be canonically released and reconciled.
No later Stado tag or deployment contains this fix.

This transport/qualification boundary is the only active blocker. Backend pinning,
activation, and production acceptance remain downstream of it.

## Changes already delivered

- PR [#118](https://github.com/wisent-ai/stado/pull/118), merge
  `d06f6cd3b52b6afb77a02925b8291e5309d805b6`, preserves the last-known-good
  release authorization during vault outages.
- The canonical Skarbiec unit owns mini loopback port 8895; the obsolete competing
  user-domain declaration was retired through the managed service surface.
- Lifecycle-scoped object/release verifier reconciliation and authenticated writer
  canary checks were added to the release flow without rotating unrelated product
  publisher items.
- PR [#129](https://github.com/wisent-ai/stado/pull/129), merge
  `a5e7ecb1c552b4e4fe38237d26bc831338d1d535`, established Stado 0.9.1 metadata
  after exact hosted qualification.
- Annotated tag `stado-v0.9.1` triggered deploy run
  [33164815481](https://github.com/wisent-ai/stado/actions/runs/33164815481).
  Its qualification and release jobs succeeded; its control-plane deployment job
  failed, so 0.9.1 was not declared as recovered production state.
- Commit `baf6f0c837d1fdbe5d08e05aa448c3d403c850be` is pushed and limited to the
  resolver transport defect proven by that failed release path.

## Timeline

| UTC | Revision / PR / run | Observed outcome |
|---|---|---|
| 2026-08-28T04:26:42Z | PR #118; merge `d06f6cd3b52b6afb77a02925b8291e5309d805b6` | Merged last-known-good release-authorization preservation. Its immediate main gate 33141829292 was superseded/cancelled. |
| 2026-08-28T10:17:32Z | PR #129 head `1046bf61dcd0c48a2fa14dac2e0c037e7d41351d`; run 33162202088 | Exact hosted version check succeeded for the 0.9.1 release PR. |
| 2026-08-28T10:18:19Z | PR #129; merge `a5e7ecb1c552b4e4fe38237d26bc831338d1d535` | Merged canonical 0.9.1 version metadata. |
| 2026-08-28T10:49:29Z–15:21:44Z | `stado-v0.9.1`; run 33164815481, attempt 6 | Qualification and immutable release publication jobs succeeded; control-plane deployment failed. |
| 2026-08-28T15:27:00Z | commit `baf6f0c837d1fdbe5d08e05aa448c3d403c850be`; PR #132 | Pushed the single canonical SSH multiplexing option fix. |
| 2026-08-28T17:04:22Z–17:05:32Z | run 33185279242, attempt 6, job 98919696309 | Candidate build completed; public baseline read failed with `error decoding response body` at `cli.storage.get`. |
| 2026-08-28T17:09:54Z | production evidence refresh | Mini port 8000 absent; bobloo root 502; public chat route 502. |

## Operator surfaces used

All host operations use Stado's declared channels; no direct SSH is used.
Representative sanctioned surfaces used during this recovery are:

```console
stado host inventory charless-mac-mini --json
stado host unit-log <target> <managed-unit>
stado host forward-remote <target> <declared-forward>
stado host reconcile-object-verifier <target> --json
stado host reconcile-release-verifier <target> --product stado --json
stado service show <service> --json
stado service status <service> --json
stado service env <service> --json
stado service grant-sync <service> <declared grant options> --json
stado storage stat <stado-uri> --json
stado storage get <stado-uri> <destination>
stado host declare-version <target> --binary stado --version <version>
stado host reconcile <target> --json
```

GitHub Actions is the only build/qualification/release pipeline. No heavy release
build is run on the operator Mac. GitHub CLI reads run, job, pull-request, and tag
evidence; it does not publish artifacts outside the repository workflow.

## Live evidence

| Criterion | Last observed UTC | Evidence | Verdict |
|---|---:|---|---|
| Mini port 8000 listens for more than two minutes | 2026-08-28T17:09:54Z | `stado host inventory charless-mac-mini --json` contained no port 8000 listener. | **FAIL** |
| `https://bobloo.com/` | 2026-08-28T17:09:54Z | Public request returned HTTP 502 in 0.242 s. | **FAIL** |
| Authenticated `/api/chat/send` returns assistant text | 2026-08-28T17:09:54Z | App/chat public route returned 502; no authenticated SSE assistant text could be produced. | **FAIL** |
| Image-router status | 2026-08-28T17:09:54Z | Not reachable for a production status observation while the app is unavailable; no paid generation was attempted. | **BLOCKED** |
| Public immutable Stado release read | 2026-08-28T17:05:32Z | Hosted run 33185279242 attempt 6 failed at `cli.storage.get`: `error decoding response body`. | **FAIL** |
| Canonical Stado/Skarbiec mini listeners | 2026-08-28T17:09:54Z | Inventory reported `127.0.0.1:8765` and `127.0.0.1:8895`; Skarbiec version matched 0.2.8. | **PASS (listener only)** |
