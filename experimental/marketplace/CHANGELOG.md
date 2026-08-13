# Changelog

All user-visible `compute-marketplace` changes are recorded here, newest first.
This repository is the `compute-marketplace` product; the publisher name and the
directory name differ.

## Unreleased

No release of this product has been published. Nothing has been released, so
this file contains no version section.

The state as observed on 2026-08-06:

- `git tag --list` and `git ls-remote --tags origin` are both empty, and the
  GitHub repository `wisent-ai/compute.wisent.com` has zero releases and zero
  tags.
- The product is a registered Stado release publisher — item
  `compute-marketplace-release-publisher`, prefix `compute-marketplace/`, per
  `wisent-compute/deploy/azure/stado.config.json` — and additionally a
  registered service deployer for `compute-marketplace-backend` and
  `compute-marketplace-frontend`. Both grants exist; nothing has been published
  through the release grant.
- Publication is a manual script, `deploy/publish_backend.sh`. No workflow
  publishes: the three registered workflows on the remote are `Agent CI`,
  `Backend CI` and `Frontend CI`, none of which references
  `publish_backend`, `stado://` or a release, and the repository's 25 workflow
  runs are all CI. Whether the script has ever been run by hand is
  **nieustalone** — the repository records no run, and the Stado object store is
  not readable from here.
- There is no single product version. `backend/Cargo.toml` declares
  `wisent-backend` `0.1.0`, `agent/Cargo.toml` declares `wisent-agent` `0.1.0`
  and `frontend/package.json` declares `compute-wisent-frontend` `0.1.0`. None
  of those versions appears in a release coordinate.
- There is no channel definition, and no `RELEASE.md`, `docs/release.md`,
  `released-surface.json` or `scripts/surface.py`.

### Product surface awaiting a first release

From `README.md`:

- A GPU compute marketplace with a Stado-backed machine control plane, shipped
  as three components selected by `MARKETPLACE_COMPONENT`: `backend` and
  `frontend` as Docker archives at
  `stado://releases/compute-marketplace/<component>/sha256/<digest>/image.tar`,
  and the Linux host-agent binary at
  `stado://releases/compute-marketplace/agent/sha256/<digest>/wisent-agent`.
- Publication resolves only `compute-marketplace-release-publisher#token`
  through its dedicated sole-item Skarbiec grant and always uses
  create-if-absent. A byte-identical retry is idempotent; different bytes at an
  existing coordinate are a hard collision.
- Deployment (`deploy/deploy_backend.sh`) downloads an image through
  `/api/release/object`, verifies the object checksum and the loaded Docker
  content ID, and restarts only the corresponding registry-managed Stado
  service through `/api/service/restart` using the separate
  `compute-marketplace-service-deployer#token`.
- Agent installation is part of the Stado machine request. There is no
  standalone host daemon, host-side API key, or provider lifecycle path.
- Backend startup is fail-closed on the `compute-marketplace-backend-runtime`
  consumer, which may read exactly five fields: `database_url`,
  `supabase_jwt_secret`, `stripe_secret_key`, `stripe_webhook_secret`,
  `stado_api_token`.
- The Stripe credit webhook authenticates Stripe's timestamped signature
  against the exact request bytes before parsing, and reserves the event id in
  `stripe_webhook_events` in the same transaction as the balance mutation, so a
  redelivered event cannot double-credit.

### Operator actions

- None. There is no published artifact to pin, verify or roll back to.

### Known limitations

- Publication requires a human running `deploy/publish_backend.sh` with the
  right component and environment; it is neither triggered nor observable from
  CI, so a release cannot be shown to be repeatable.
- The release coordinate carries only a digest — no version and no channel — so
  the promotion, compatibility and rollback contract required by the versioning
  guidelines is undefined for this product.
- The working tree diverges from `origin/main` and part of the deployment path
  described in `README.md` is not committed: `deploy/deploy_backend.sh` and
  `supabase/migrations/00006_stripe_webhook_events.sql` exist only as untracked
  files. `deploy/publish_backend.sh` is committed and present on `origin/main`.
