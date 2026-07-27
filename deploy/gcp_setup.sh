#!/bin/bash
set -euo pipefail

PROJECT="${GCP_PROJECT:?GCP_PROJECT required}"
REGION="${GCP_REGION:-us-central1}"
BUCKET="stado"
SA_NAME="wisent-compute-sa"
SA_EMAIL="${SA_NAME}@${PROJECT}.iam.gserviceaccount.com"
SERVICE="stado-coordinator"
SCHEDULER="wisent-compute-cron"
TOPIC="wisent-compute-alerts"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== Deploying wisent-compute ==="

# 1. GCS bucket
if ! gsutil ls -b "gs://${BUCKET}" >/dev/null 2>&1; then
    gsutil mb -l "$REGION" "gs://${BUCKET}"
fi
echo "Bucket: gs://${BUCKET}"

# 2. Upload quotas
gsutil cp "$REPO_DIR/config/quotas.json" "gs://${BUCKET}/config/quotas.json"
echo "Quotas uploaded"

# 3. Service account
if ! gcloud iam service-accounts describe "$SA_EMAIL" --project="$PROJECT" >/dev/null 2>&1; then
    gcloud iam service-accounts create "$SA_NAME" --project="$PROJECT" --display-name="Wisent Compute"
fi
# bigquery.jobUser lets the tick run the billing-export queries; dataViewer
# lets it read the gcp_billing_export_v1_* table the credits collector reads.
# secretAccessor (already listed) covers the optional Azure billing SP secret
# wisent-azure-billing-sp consumed by the same collector — no extra binding
# needed for the Azure path, it activates automatically once that secret
# exists. This keeps credit tracking fully automated with no manual IAM step.
for role in roles/compute.admin roles/storage.admin roles/pubsub.publisher roles/secretmanager.secretAccessor roles/bigquery.jobUser roles/bigquery.dataViewer; do
    gcloud projects add-iam-policy-binding "$PROJECT" \
        --member="serviceAccount:${SA_EMAIL}" --role="$role" --quiet >/dev/null 2>&1
done
echo "Service account: $SA_EMAIL"

# 4. Pub/Sub
if ! gcloud pubsub topics describe "$TOPIC" --project="$PROJECT" >/dev/null 2>&1; then
    gcloud pubsub topics create "$TOPIC" --project="$PROJECT"
fi
echo "Alerts topic: $TOPIC"

# 5. Secrets
for secret in wisent-hf-token wisent-gh-token; do
    if ! gcloud secrets describe "$secret" --project="$PROJECT" >/dev/null 2>&1; then
        echo "Create secret: echo -n '\$TOKEN' | gcloud secrets create $secret --data-file=- --project=$PROJECT"
    fi
done

# 6. Publish release binaries and deploy the Rust Cloud Run coordinator.
# The deploy script also grants the scheduler invoker role, repoints the
# existing cron health check, and removes the retired Python function.
GCP_PROJECT="$PROJECT" GCP_REGION="$REGION" bash "$SCRIPT_DIR/deploy_stado_rust.sh"
echo "Rust Cloud Run coordinator deployed"

echo ""
echo "=== wisent-compute deployed ==="
echo "Bucket:    gs://${BUCKET}"
echo "Service:   $SERVICE"
echo "Scheduler: $SCHEDULER"
echo "Alerts:    $TOPIC"
