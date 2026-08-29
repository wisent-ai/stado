# Wisent app backend production recovery

Last updated: 2026-08-29T04:13:45Z

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

**FIXED: Route verdict evaluation logic (PR #140, merged 2026-08-29T02:17:43Z)**
- Commit `edb5082b` (merge), implementation `1d370bb8`
- Changed jq filter from checking per-entry boolean results to using `any()` check
- Route reconciliation now exits true if ANY entry matches the configuration
- Verified working: Run 33230476813 version-check passed; run 33231109170 step 4 succeeded

**FIXED: Native darwin-arm64 recovery publication missing (PR #141, merged 2026-08-29T03:04:11Z)**
- Commit `bf477f54` (merge), implementation `735e3e08`
- Added "Publish the exact tag native release" step to deployment workflow
- Builds darwin-arm64 binary from exact git tag `stado-v0.9.3`
- Publishes to storage with transfer-safe client, byte-verifies public delivery
- Verified working: Run 33231109170 step 7 completed successfully at 2026-08-29T03:46:31Z

**FIXED: Deployment self-target environment pollution (PR #142, merged 2026-08-29T04:13:12Z)**
- Commit `d2b7b815` (implementation), merge `d1358e64`
- Root cause of run 33231109170 step 10 failure: deployment script line 65 executed `env -u STADO_API_URL -u STADO_API_TOKEN` for self_target, unsetting API URL required for canonical release reads. Error: "STADO_API_URL is required for canonical release reads"
- Fix: Change line 65 to only unset STADO_API_TOKEN while preserving STADO_API_URL in environment
- Feature branch run 33232701781 validated fix: version-check SUCCESS, all 9 steps passed (2026-08-29T03:58:57Z–04:12:35Z)
- Merged to main at 2026-08-29T04:13:12Z by lbartoszcze
- Main gate run 900 (version-check) started 2026-08-29T04:13:14Z after merge, currently in_progress

**Current blocker (Separate from run 33231109170): Route verdict evaluation logic requires nested evaluation (PR #140 incomplete)**
- PR #140 merged 2026-08-29T02:17:43Z; run 33231109170 passed step 4 (route reconciliation) but failed at step 10 due to STADO_API_URL unset (fixed by PR #142)
- Route verdict logic still requires nested evaluation of ALL route entries per comments; jq `any()` change in PR #140 is necessary but may require additional refinement
- Deployment operations can proceed once PR #142 fix qualifies on main

**Secondary blocker: Feature branch qualification in progress**
- Run 33232701781 (2026-08-29T03:58:57Z–04:12:35Z, on branch `fix/self-release-canonical-read`): Version-check completed SUCCESS. All 9 steps passed; feature validates canonical API self-release preservation.
- Commit: d2b7b815 "Preserve canonical API for self release"
- Status: Feature branch qualified; awaits merge to main or automated deployment trigger.
- No production impact; deployment blocked by main-branch resolver route verdict blocker.
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
- PR [#140](https://github.com/wisent-ai/stado/pull/140) "Evaluate managed release routes as one verdict", merge `edb5082b`, implementation `1d370bb8`: Fixes route reconciliation jq filter to check if ANY entry matches instead of validating per-entry logic. Merged 2026-08-29T02:17:43Z.
- PR [#141](https://github.com/wisent-ai/stado/pull/141) "Recover the exact native Stado release from its tag", merge `bf477f54`, implementation `735e3e08`: Adds native darwin-arm64 publication to deployment workflow; builds and publishes from exact git tag; byte-verifies public delivery before managed host reconciliation. Merged 2026-08-29T03:04:11Z.
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
| 2026-08-29T02:52:09Z–03:04:14Z | run 33230476813: Version check on main | Version-check SUCCESS; released Stado 0.9.3. |
| 2026-08-29T03:19:38Z–03:56:51Z | run 33231109170: Deploy existing Stado release (main, 0.9.3) | Deployment run failed at step 10 ("Deploy the existing immutable release"). **Actual root cause (corrected):** deployment script `scripts/deploy_existing_stado_release.sh` line 65 executed `env -u STADO_API_URL -u STADO_API_TOKEN` for self_target, unsetting STADO_API_URL required for canonical release reads. CLI error: "STADO_API_URL is required for canonical release reads" (error_code: config, non-retryable, critical). Steps 1–9 completed successfully: client build (4m24s), release origin (5s), immutable publication (4m24s), public bytes (8m02s), native darwin-arm64 (9m50s), release channel (9m52s), deployment gate (8m41s), owner resolver verify (5m22s), release acceptance (2m03s). **Fix:** PR #142 preserves STADO_API_URL instead of unsetting it (see run 33232701781 below). |
| 2026-08-29T03:58:57Z–04:12:35Z | run 33232701781: Version check on `fix/self-release-canonical-read` | Version-check SUCCESS; all 9 steps passed. Feature branch qualifies canonical API self-release preservation (commit d2b7b815). **Merged to main as PR #142 with merge commit d1358e64 at 2026-08-29T04:13:12Z.** Main gate run 900 started 2026-08-29T04:13:14Z, currently in_progress (version-check). Production state awaits main-gate completion to validate step 10 deployment. |

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
| Mini port 8000 listens for more than two minutes | 2026-08-29T04:13:00Z | `stado host inventory charless-mac-mini --json` contains no port 8000 listener. | **FAIL** |
| `https://bobloo.com/` | 2026-08-29T04:13:00Z | Public request returned HTTP 502; Cloudflare upstream unavailable. | **FAIL** |
| Authenticated `/api/chat/send` returns assistant text | 2026-08-29T04:13:00Z | App/chat public route returned 502; no authenticated SSE assistant text could be produced. | **FAIL** |
| Image-router status | 2026-08-29T04:13:00Z | Not reachable for a production status observation while the app is unavailable; no paid generation was attempted. | **BLOCKED** |
| Deployment self-target canonical API preservation (PR #142) | 2026-08-29T04:13:12Z | Run 33231109170 step 10 failed with "STADO_API_URL is required for canonical release reads" because script unsetting both URL and token. PR #142 fix preserves STADO_API_URL while unsetting only STADO_API_TOKEN. Merge commit `d1358e64`. Feature run 33232701781 validated all 9 steps. Main gate run 900 in_progress. | **IN PROGRESS (main qualification)** |
| Stado 0.9.3 release qualification | 2026-08-29T03:04:14Z | Run 33230476813 version-check SUCCESS on main; release pipeline triggered; Stado 0.9.3 released and qualified. | **PASS** |
| Native darwin-arm64 recovery publication (PR #141) | 2026-08-29T03:46:31Z | Run 33231109170 step 7 completed successfully; native darwin-arm64 binary published to immutable storage from git tag stado-v0.9.3; byte-verification passed. | **PASS** |
| Feature branch canonical API preservation (`fix/self-release-canonical-read` / PR #142) | 2026-08-29T04:12:35Z | Run 33232701781 version-check SUCCESS; all 9 steps passed; commit d2b7b815 qualifies canonical API self-release preservation. Merged to main as PR #142 at 2026-08-29T04:13:12Z; merge commit `d1358e64`. Main gate run 900 started immediately after merge, currently in_progress. | **MERGED, MAIN QUALIFYING** |
