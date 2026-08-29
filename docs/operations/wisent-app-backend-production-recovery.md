# Wisent App backend production recovery

Last updated: 2026-08-29T11:23:32Z

## Acceptance criteria

| Criterion | Result | Evidence |
| --- | --- | --- |
| Managed Stado release is reconciled | PASS | Deployment run `33239124143` completed successfully; Stado `0.9.3` is active and in sync on the declared targets with the recorded platform digests below. |
| Backend listens stably on the mini | PASS | PID `49472` listened on `127.0.0.1:8000` in two observations 155 seconds apart; Uvicorn was ready and process stderr was empty. |
| Public backend origin is healthy | PASS | `https://bobloo.com/` returned HTTP 200. |
| Direct authenticated chat works | PASS | Authenticated `/api/chat` returned HTTP 200 SSE with assistant text `I'm here and ready to help with anything you need! 😊` and no embedded error. |
| App authenticated chat works | PASS | Authenticated `POST https://app.wisent.ai/api/chat/send` returned HTTP 200 `text/event-stream`, assistant text `Production app chat is healthy.`, and no embedded error. |
| Image routing is healthy without a paid generation | PASS | The declared `image-video-router` service runs on `ubuntu-server-rtx-pro-6000`; product-recorded service verification observed its declared endpoint. No image generation was invoked. |
| Diagnostic state is removed | PASS | Temporary local forwards on ports `19000` and `19001` were stopped; neither port had a listener afterward. |

## Current production state

Production is recovered. There is no remaining recovery blocker.

The backend is running from the immutable Wisent backend release, the public origin and app alias return HTTP 200, and both direct and app-mediated authenticated chat streams return real assistant text without embedded errors. The app's canonical Vercel production declaration uses `AI_SERVICE_URL=https://bobloo.com`; the superseded private Tailnet URL is no longer the production value.

## Immutable releases

| Component | Immutable URI | SHA-256 / content digest |
| --- | --- | --- |
| Stado 0.9.3 Darwin arm64 | `stado://releases/stado/0.9.3/darwin-arm64/stado-v0.9.3-darwin-arm64.tar.gz` | `e5e6e26d255a9a7170af74179395532dcfa55a4fa3ea834a11f872bded01251d` |
| Stado 0.9.3 Linux amd64 | `stado://releases/stado/0.9.3/linux-amd64/stado-v0.9.3-linux-amd64.tar.gz` | `96b800dcfde67a908e5a08abb8af57ef2ea1d492b00f8c1218554b24b93acadf` |
| Wisent backend | `stado://releases/wisent-backend/sha256/065e8133abbd96b212f538c983a940381971610a150383887e50fa6de8f66287/release-manifest.json` | `065e8133abbd96b212f538c983a940381971610a150383887e50fa6de8f66287` |

Stado `0.9.3` was built from source commit `54e165bdf26adef1e6b8b2ac998f9cce891a7530`.

## Vercel production deployment

| Field | Value |
| --- | --- |
| Source deployment | `dpl_9PvBHZjNQemzH5fQCtk7Y6wakqG3` |
| Current production deployment | `dpl_66zZ3pCbDZzm7ookjnZRJ7bHKYps` |
| Deployment URL | `https://wisent-jrbb7ei0w-my-team-c19efe71.vercel.app` |
| Production alias | `https://app.wisent.ai` |
| Canonical backend declaration | `AI_SERVICE_URL=https://bobloo.com` |
| Canonical source configuration | `wisent-ai/wisent-app` commit [`e426105443a13da2a6e590c2a299311ab05416e7`](https://github.com/wisent-ai/wisent-app/blob/e426105443a13da2a6e590c2a299311ab05416e7/vercel.json), `vercel.json` |
| State | Ready |

## Recovery timeline

All timestamps are UTC.

| Time | Event |
| --- | --- |
| 2026-08-29T06:42:39Z | Canonical Stado `0.9.3` deployment run `33239124143` started. |
| 2026-08-29T07:02:19Z | The run recorded successful public verification of the native release bytes. |
| 2026-08-29T07:11:15Z | Run `33239124143` completed successfully with the declared targets in sync. |
| 2026-08-29T10:59:14Z | Final production verification completed: stable backend PID/listener, bobloo and app HTTP 200, direct and app authenticated assistant SSE without embedded errors, image-router service evidence, and diagnostic-forward cleanup. |
| 2026-08-29T11:23:32Z | Committed the canonical `AI_SERVICE_URL=https://bobloo.com` declaration to the linked `wisent-ai/wisent-app` production repository at `e426105443a13da2a6e590c2a299311ab05416e7`; the already-ready production deployment was left unchanged. |

## Changes delivered

- Published and verified the immutable Stado `0.9.3` Darwin arm64 and Linux amd64 release channels through the canonical release pipeline.
- Reconciled the declared managed Stado targets to the exact platform releases and digests.
- Restored the backend release runner's authenticated access to its immutable Stado release manifest.
- Activated the immutable backend digest `065e8133abbd96b212f538c983a940381971610a150383887e50fa6de8f66287`.
- Restored the declared ingress path to the healthy backend listener.
- Set the canonical Vercel production `AI_SERVICE_URL` to `https://bobloo.com` and redeployed the existing production revision through the established Vercel path.
- Recorded `AI_SERVICE_URL=https://bobloo.com` in the linked production repository's canonical `vercel.json` so future deployments cannot restore the superseded private Tailnet origin.
- Removed the obsolete diagnostic local forwards after declared production paths passed.

## Operator surfaces used

Host and service operations used Stado's declared surfaces; no direct SSH was used. Representative read-only verification surfaces included:

```text
stado service show --host <declared-host> <declared-service>
stado service verify --host ubuntu-server-rtx-pro-6000 --json image-video-router
stado host forward-remote <declared-host> ...
```

The Vercel production environment and existing production revision were managed through the repository's established Vercel CLI/deployment path. Authentication material was supplied through managed credential boundaries and was not placed in command arguments or recorded here.

## Live evidence

| Surface | Final observation |
| --- | --- |
| Backend listener | PID `49472`, `127.0.0.1:8000`, same PID after 155 seconds; Uvicorn ready; empty stderr. |
| Backend immutable manifest | Authenticated read succeeded for the exact backend URI and digest recorded above. |
| bobloo | `https://bobloo.com/` returned HTTP 200. |
| Direct authenticated chat | HTTP 200 SSE, real assistant text, no embedded error. |
| App root | `https://app.wisent.ai/` returned HTTP 200. |
| App authenticated chat | HTTP 200 `text/event-stream`, assistant text `Production app chat is healthy.`, `embeddedError=false`. |
| Image router | `image-video-router-release.service` reported `runs`; startup record identified version `0.1.0`, host `ubuntu-server-rtx-pro-6000`, GPU `cuda:0`, lease GPU `nvidia-rtx-pro-6000-bse`, port `14102`, and managed model server enabled. Service verification observed the declared `http://127.0.0.1:8081` endpoint; its root returned HTTP 404, establishing that the routed service answered without incurring generation cost. |
| Diagnostic cleanup | Verified diagnostic forward processes were stopped; local ports `19000` and `19001` had no listeners afterward. |
