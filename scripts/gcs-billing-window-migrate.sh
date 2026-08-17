#!/bin/bash
# Copy the manifest-bound Wisent media out of Cloud Storage inside the shortest
# possible attached-billing window, then detach billing again.
#
# Project wisent-480400 is deliberately billing-detached. Object bodies cannot
# be read in that state, and no other route exists: Requester Pays cannot be
# enabled (the bucket write is itself refused), the Cloud Support channel is
# refused, the Firebase project holds no copy, and the AWS origin account is
# banned. So the window is opened on purpose, kept to minutes, and closed by a
# trap that fires on success, failure and interrupt alike.
#
# The standing rate of the attached project is about $1.40/hour of storage,
# disks, custom images and reserved addresses, prorated to the sub-second, plus
# $0.12/GiB of egress. Leaving billing attached is the expensive mistake this
# script exists to make impossible.
set -uo pipefail

GCLOUD=${GCLOUD:-/opt/homebrew/share/google-cloud-sdk/bin/gcloud}
PROJECT=${PROJECT:-wisent-480400}
BILLING_ACCOUNT=${BILLING_ACCOUNT:?BILLING_ACCOUNT is required}
OPERATOR=${OPERATOR:-lukasz.bartoszcze@gmail.com}
READER=${READER:-droid-441@wisent-480400.iam.gserviceaccount.com}
ACCOUNT=${ACCOUNT:-wisentprodstado}
MANIFEST=${MANIFEST:-migration/wisent-images-live-locators-2026-08-17.json}
REPORT_DIR=${REPORT_DIR:-migration}

started=$(date -u +%s)
detached=no

detach() {
    if [ "$detached" = yes ]; then
        return
    fi
    detached=yes
    printf '\n=== detaching billing ===\n'
    for attempt in 1 2 3 4 5; do
        if "$GCLOUD" billing projects unlink "$PROJECT" \
            --account="$OPERATOR" --quiet >/dev/null 2>&1; then
            break
        fi
        printf 'unlink attempt %s failed; retrying\n' "$attempt" >&2
        sleep 5
    done
    state=$("$GCLOUD" billing projects describe "$PROJECT" --account="$OPERATOR" \
        --format='value(billingEnabled)' --quiet 2>/dev/null)
    elapsed=$(( $(date -u +%s) - started ))
    printf 'billingEnabled=%s window_seconds=%s\n' "${state:-unknown}" "$elapsed"
    if [ "$state" != "False" ]; then
        printf 'BILLING IS STILL ATTACHED - detach by hand now:\n  %s billing projects unlink %s\n' \
            "$GCLOUD" "$PROJECT" >&2
    fi
}
trap detach EXIT HUP INT TERM

printf '=== attaching billing %s to %s ===\n' "$BILLING_ACCOUNT" "$PROJECT"
"$GCLOUD" billing projects link "$PROJECT" --billing-account="$BILLING_ACCOUNT" \
    --account="$OPERATOR" --quiet --format='value(billingEnabled)' || exit 1

for attempt in 1 2 3 4 5 6; do
    if "$GCLOUD" storage cat "gs://wisent-images-bucket/images/characters/8808.webp" \
        --account="$READER" --quiet >/dev/null 2>&1; then
        printf 'source bodies readable after %s check(s)\n' "$attempt"
        break
    fi
    printf 'waiting for billing to propagate (%s)\n' "$attempt"
    sleep 10
done

status=0
run_pass() {
    container=$1
    report=$2
    shift 2
    printf '\n=== copying into %s ===\n' "$container"
    python3 scripts/migrate-media-manifest-to-azure.py \
        --manifest "$MANIFEST" \
        --account "$ACCOUNT" \
        --container "$container" \
        --gcloud-account "$READER" \
        --concurrency 16 \
        --report "$REPORT_DIR/$report" \
        "$@" || status=1
}

run_pass media-public copy-report-media-public.json \
    --only-prefix images/characters/ --only-prefix images/seed/ \
    --only-prefix images/generated/ --only-prefix images/rooms/
run_pass media-private copy-report-media-private.json \
    --only-prefix images/profiles/ --only-prefix profiles/

printf '\n=== verifying destination against manifest ===\n'
python3 scripts/migrate-media-manifest-to-azure.py --manifest "$MANIFEST" \
    --account "$ACCOUNT" --container media-public --dry-run \
    --only-prefix images/characters/ --only-prefix images/seed/ \
    --only-prefix images/generated/ --only-prefix images/rooms/ | head -2 || status=1
python3 scripts/migrate-media-manifest-to-azure.py --manifest "$MANIFEST" \
    --account "$ACCOUNT" --container media-private --dry-run \
    --only-prefix images/profiles/ --only-prefix profiles/ | head -2 || status=1

exit "$status"
