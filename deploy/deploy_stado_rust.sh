#!/bin/bash
set -euo pipefail

# CI-friendly redeploy: only the steps the wisent-compute-sa is permitted
# to do. For one-time bootstrap (creating the SA, granting project roles,
# creating the bucket and pub/sub topic), use gcp_setup.sh instead.

PROJECT="${GCP_PROJECT:?GCP_PROJECT required}"
REGION="${GCP_REGION:-us-central1}"
BUCKET="stado"
SA_EMAIL="wisent-compute-sa@${PROJECT}.iam.gserviceaccount.com"
SERVICE="stado-coordinator"
SCHEDULER="wisent-compute-cron"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== Publishing and deploying Rust stado coordinator ==="

# 1. Upload quotas (storage.admin)
gsutil cp "$REPO_DIR/config/quotas.json" "gs://${BUCKET}/config/quotas.json"
echo "Quotas refreshed at gs://${BUCKET}/config/quotas.json"


# Build release binaries, publish the GCS self-update tree, and push the
# Cloud Run image. Set STADO_SKIP_BUILD=yes only when resuming after this
# exact source revision has already published successfully.
if [[ "${STADO_SKIP_BUILD:-}" != "yes" ]]; then
    gcloud builds submit "$REPO_DIR/stado-rs" \
        --config="$REPO_DIR/stado-rs/cloudbuild.yaml" \
        --project="$PROJECT" --quiet
fi

VERSION_LINE="$(grep '^version' "$REPO_DIR/stado-rs/Cargo.toml")"
VERSION="${VERSION_LINE#*\"}"
VERSION="${VERSION%%\"*}"
IMAGE="us-central1-docker.pkg.dev/${PROJECT}/stado/stado-coordinator:${VERSION}"
MIN_INSTANCES="$(printf x | /usr/bin/wc -c | tr -d ' ')"
INTERVAL_SECONDS="${STADO_INTERVAL_SECONDS:-180}"
gcloud run deploy "$SERVICE" \
    --image="$IMAGE" --region="$REGION" \
    --service-account="$SA_EMAIL" \
    --command=/out/stado --args=cloud-control-plane,--interval="$INTERVAL_SECONDS" \
    --min-instances="$MIN_INSTANCES" --max-instances="$MIN_INSTANCES" \
    --no-cpu-throttling --no-allow-unauthenticated \
    --set-secrets="HF_TOKEN=wisent-hf-token:latest,HUGGING_FACE_HUB_TOKEN=wisent-hf-token:latest" \
    --set-env-vars="GCP_PROJECT=${PROJECT},WC_BUCKET=${BUCKET},WC_ALERTS_TOPIC=projects/${PROJECT}/topics/wisent-compute-alerts,WC_COORDINATOR_ID=rust-cloud-run,STADO_DEPLOYMENT_ID=cloud-run,WC_SLACK_WEBHOOK=${WC_SLACK_WEBHOOK:-},WC_TELEGRAM_BOT_TOKEN=${WC_TELEGRAM_BOT_TOKEN:-},WC_TELEGRAM_CHAT_ID=${WC_TELEGRAM_CHAT_ID:-},WC_SENDGRID_API_KEY=${WC_SENDGRID_API_KEY:-},WC_EMAIL_TO=${WC_EMAIL_TO:-},WC_EMAIL_FROM=${WC_EMAIL_FROM:-compute@example.com}" \
    --project="$PROJECT" --quiet

echo "Cloud Run service $SERVICE deployed"

gcloud run services add-iam-policy-binding "$SERVICE" \
    --region="$REGION" --project="$PROJECT" \
    --member="serviceAccount:${SA_EMAIL}" --role="roles/run.invoker" --quiet >/dev/null

# Point the existing scheduler at the Rust service's authenticated liveness
# endpoint. Scheduling itself runs continuously in the single warm instance.
URL=$(gcloud run services describe "$SERVICE" --region="$REGION" --project="$PROJECT" --format='value(status.url)')
if gcloud scheduler jobs describe "$SCHEDULER" --location="$REGION" --project="$PROJECT" >/dev/null 2>&1; then
    gcloud scheduler jobs update http "$SCHEDULER" --location="$REGION" --project="$PROJECT" \
        --schedule="*/3 * * * *" --uri="${URL}/livez" --http-method=GET \
        --oidc-service-account-email="$SA_EMAIL" --oidc-token-audience="$URL" --quiet
    echo "Scheduler $SCHEDULER updated"
else
    gcloud scheduler jobs create http "$SCHEDULER" --location="$REGION" --project="$PROJECT" \
        --schedule="*/3 * * * *" --uri="${URL}/livez" --http-method=GET \
        --oidc-service-account-email="$SA_EMAIL" --oidc-token-audience="$URL" --quiet
    echo "Scheduler $SCHEDULER created"
fi

# Clean cutover: the scheduler now targets Cloud Run and the continuous Rust
# coordinator owns ticks, so the retired Python function must not restart.
gcloud functions delete wisent-compute-tick \
    --gen2 --region="$REGION" --project="$PROJECT" --quiet || true

echo "=== Rust coordinator deploy complete ==="
