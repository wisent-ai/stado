# Wisent app backend production recovery

Last updated: 2026-08-29T01:03:00Z

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

**FIXED: SSH multiplexing issue** (PR #132, merged 2026-08-28T20:45:11Z):
Commit `baf6f0c837d1fdbe5d08e05aa448c3d403c850be` removed contradictory `ControlMaster=no`
override, reused one canonical SSH option construction. Main qualification run
[33209574217](https://github.com/wisent-ai/stado/actions/runs/33209574217)
succeeded at 2026-08-28T20:59:26Z. Triggered release pipeline.

**Current blocker: Stado 0.9.3 partial recovery publication workflow exists but CLI subcommand `recover-object-api` not implemented in running version 0.7.34.**

**Completed: Writer read-back transport layer recovered (run 33220762292, 2026-08-28T23:38:19Z)**
- Branch: fix/resumable-writer-readback; head: a2f35bf3 "ci: avoid stado wc alias in transfer proof"
- Job: release-sized-read-back → SUCCESS
- Evidence: Full release-sized Stado object successfully transferred through authenticated writer loopback; byte-for-byte transfer verified
- PR #136 "Resume authenticated Stado writer read-back" merged at 2026-08-28T23:47:06Z (commit b699b63f)

**Completed: Stado 0.9.3 partial immutable release recovery workflow created (run 33224524061, 2026-08-29T01:02:24Z)**
- PR #137 "Resume partial immutable Stado releases" merged at 2026-08-29T00:46:30Z (commit 7d2e1ee6, merge f03db782)
- Commit timestamp: 2026-08-29T00:30:24Z by lbartoszcze
- Changes: .github/workflows/deploy-existing-release.yml (workflow supports idempotent publication with existing tag)
- Workflow description: "Completes an interrupted immutable release only from its existing tag-bound archive, verifies its manifest and member digests, publishes missing objects idempotently with the current transfer-safe client, verifies public bytes, and then invokes the established managed deployment path."
- Merge qualification run 33224524061: version-check → SUCCESS (2026-08-29T01:02:24Z, completed 01:02:24Z)

**NEW BLOCKER: Deployment triggered immediately after version-check pass (run 33225312766, 2026-08-29T01:02:51Z → 01:02:59Z)**
- Run: 33225312766 "deploy existing Stado release" → FAILURE
- Failed step: "Reconcile public release origin" (exit code 2, 8 seconds)
- Error message: `error: unrecognized subcommand 'recover-object-api'` from stado CLI
- Root cause: The stado CLI binary installed locally (version 0.7.34) does not expose the `recover_object_api` subcommand. The function exists in source code (stado-rs/src/cli/host.rs) but is not yet exposed as a public CLI subcommand.
- Expected vs actual: Workflow expects `stado host recover-object-api charless-mac-mini --json` but CLI lacks this subcommand.
- Release tag stado-v0.9.3 exists (SHA 925a03be7be273a4c95c14f666ff37a58fda4313, created by run 33216246052 at 22:18:11Z)
- No retry triggered after deployment failure. Production state: port 8000 absent, bobloo HTTP 502, chat HTTP 502.

**Next step:** CLI subcommand must be added to stado-rs and published before deployment can proceed.
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
| 2026-08-28T17:16:57Z–17:17:20Z | managed owner resolver restart | Resolver reported `ready`; three consecutive public release-object stats returned authoritative responses. |
| 2026-08-28T17:22:34Z–17:23:37Z | run 33193485453, head `9855235f167d8725ef9c417646e6c15d66941a29` | Public immutable baseline transfer again truncated with `error decoding response body`. |
| 2026-08-28T17:27:30Z | mini SSH pressure measurement | Sanctioned host process report contained 78 SSH-related processes. |
| 2026-08-28T17:30:03Z–17:37:25Z | managed resolver quiesce | `stado service stop` removed the owner resolver unit process; mini SSH-related process count drained to a stable baseline of 17. |
| 2026-08-28T17:53:14Z | run 33195644902, head `654e1163b932217cd759e927bfceee027229d1f6` | Public immutable baseline transfer truncated with the same `error decoding response body` fault. Mini SSH pressure remains stable at low baseline. |
| 2026-08-28T18:03:02Z | production evidence refresh | Mini port 8000 remains absent; bobloo root and chat route return 502; managed resolver remains quiesced. |
| 2026-08-28T19:00:00Z (approx) | PR #132 head `19d2b5d0` pushed; temporary diagnostic forward declared | `release-gate-bootstrap` forward local 18776→mini 8765 enabled for loopback testing; public ingress charless-mac-mini.tail6443b3.ts.net:8443→127.0.0.1:18081 declared. |
| 2026-08-28T19:15:00Z (approx) | loopback stat test within forward | Stado object API at 127.0.0.1:8765 responding to stat requests; public DNS/byte identity unproven at read boundary. |
| 2026-08-28T19:30:00Z (approx) | run 33200362488, head `19d2b5d0` | Public immutable baseline read truncated with same `error decoding response body` fault; internal loopback succeeded but public path failed. 0.9.1 attempt 6 had failed at first PUT 502 and was not rerun. |
| 2026-08-28T20:27:06Z | PR #132 head `9efa218c` pushed; run 33208281366 created | New gate run with updated PR head started; previous run 33200362488 failed. |
| 2026-08-28T20:31:54Z | production evidence refresh; gate in progress | Mini port 8000 remains absent; bobloo root and chat route return 502; gate 33208281366 currently in_progress (version-check). |
| 2026-08-28T20:45:11Z | PR #132 merged to main (SSH multiplexing fix) | Merge commit `28a5ddfb62a74cfb6524368d846f3af3737ea20f` merged; resolver SSH multiplexing fix now on main branch. |
| 2026-08-28T20:59:26Z | run 33209574217 on main, merge commit `28a5ddfb` | Version-check succeeded on main branch; SSH multiplexing fix qualified cleanly; release pipeline triggered. |
| 2026-08-28T21:02:02Z–21:11:21Z | run 33210783452: Release Stado 0.9.2 | Release pipeline triggered after main qualification; version-check job succeeded. Tag stado-v0.9.2 (sha: 41456b1f) created. |
| 2026-08-28T21:11:46Z | PR #133 merged: Release Stado 0.9.2 | Release branch merged to main; deployment pipeline activated. |
| 2026-08-28T21:11:49Z–21:21:15Z | run 33211496795: Merge PR #133 | Release merge run completed with success conclusion. |
| 2026-08-28T21:21:55Z–21:41:56Z | run 33212229611: Release deployment (0.9.2) | Deployment attempt after 0.9.2 release. Release job failed in "Validate public Linux release delivery" step; HTTP 502 from Stado object API for `stado://releases/stado/0.9.2/linux-amd64/SHA256SUMS` (21:39:07Z–21:41:52Z). Failure details: "Stado object API returned HTTP 502 Bad Gateway: <empty response body>" with retries up to 12 times, error_code=infra_down. Deploy-control-plane job skipped due to release failure. Production unchanged (port 8000 absent, bobloo/chat 502). |
| 2026-08-28T22:00:51Z | PR #134 merged: Mount Stado release delivery beside mini ingress | Fixes the 0.9.2 deployment failure (run 33212229611) by reconciling path-scoped public release route on mini's existing 443 Funnel. Merge commit `3eb5465367e8177159e6b14efa945767fe9ad946`. |
| 2026-08-28T21:46:28Z–22:00:23Z | run 33214002442: PR #134 merge qualification | PR branch run completed with success conclusion before merge. |
| 2026-08-28T22:00:54Z–in_progress | run 33215035738: Merge qualification for PR #134 | Post-merge qualification run started; version-check job in_progress (started 22:00:54Z, still running at 22:15:58Z). Once this qualifies, release deployment will be triggered again. |

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
stado service stop <service> --host <target> --listener-url <loopback-url> --json
stado service restart <service> --host <target> --json
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
| `https://bobloo.com/` | 2026-08-28T22:15:58Z | Public request returned HTTP 502; Cloudflare upstream unavailable. | **FAIL** |
| Authenticated `/api/chat/send` returns assistant text | 2026-08-28T22:15:58Z | App/chat public route returned 502; no authenticated SSE assistant text could be produced. | **FAIL** |
| Image-router status | 2026-08-28T17:09:54Z | Not reachable for a production status observation while the app is unavailable; no paid generation was attempted. | **BLOCKED** |
| SSH multiplexing fix (PR #132) | 2026-08-28T20:59:26Z | PR #132 merged at 20:45:11Z; run 33209574217 on main succeeded with version-check COMPLETE/SUCCESS; fix qualified cleanly and triggered release pipeline. | **PASS** |
| Stado 0.9.2 release tag | 2026-08-28T21:11:46Z | Tag stado-v0.9.2 (sha: 41456b1f) created; Release run 33210783452 completed SUCCESS (21:02:02Z-21:11:21Z); PR #133 merged 21:11:46Z. | **PASS** |
| 0.9.2 deployment failure and Funnel-path fixes | 2026-08-28T22:15:58Z | Run 33212229611 failed (21:22:08Z-21:41:56Z) in "Validate public Linux release delivery" with HTTP 502 from Stado object API; recorded failure: "Stado object API returned HTTP 502 Bad Gateway: <empty response body>" trying to serve `stado://releases/stado/0.9.2/linux-amd64/SHA256SUMS`. Fixes in flight: PR #131 "Preserve release path through Funnel" (merged 14:00:45Z, run 33177085951 SUCCESS) and PR #134 "Mount Stado release delivery beside mini ingress" (merged 22:00:51Z, run 33214002442 SUCCESS). Merge qualification run 33215035738 in_progress (version-check, started 22:00:54Z). | **IN PROGRESS** |
| Loopback stat via temporary diagnostic forward | 2026-08-28T19:15:00Z | Forward `release-gate-bootstrap` (18776→8765) enabled; Stado object API at 127.0.0.1:8765 responded to stat requests successfully. | **PASS** |
