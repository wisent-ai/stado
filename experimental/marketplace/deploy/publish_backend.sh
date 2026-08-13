#!/bin/sh
set -eu

: "${STADO_API_URL:?inject the Stado object API URL}"
: "${STADO_PUBLISHER_SKARBIEC_URL:?inject the Skarbiec URL for the dedicated marketplace publisher}"
case "$STADO_API_URL" in
    https://*|http://localhost:*) ;;
    *) echo "STADO_API_URL must use HTTPS or an authenticated loopback endpoint"; false ;;
esac
stado_api_url="${STADO_API_URL%/}"

repo_root="${REPO_ROOT:-$(pwd)}"
stado_bin="${STADO_BIN:-stado}"
export WC_SKARBIEC_URL="$STADO_PUBLISHER_SKARBIEC_URL"
export WC_SKARBIEC_CONSUMER="compute-marketplace-release-publisher"
export WC_SKARBIEC_TOKEN_FILE="${STADO_PUBLISHER_SKARBIEC_TOKEN_FILE:-$HOME/.stado/compute-marketplace-release-publisher-skarbiec-token}"
if [ ! -f "$WC_SKARBIEC_TOKEN_FILE" ]; then
    echo "dedicated marketplace release-publisher grant is unavailable"
    false
fi
stado_api_token="$("$stado_bin" secrets get compute-marketplace-release-publisher --field token)"
: "${stado_api_token:?Skarbiec returned an empty Stado publisher token}"
component="${MARKETPLACE_COMPONENT:-backend}"
case "$component" in
    backend)
        : "${STADO_RELEASE_URL:?set the pinned Stado CLI binary URL used by the image build}"
        : "${STADO_RELEASE_SHA256:?set the pinned Stado CLI binary checksum}"
        dockerfile="$repo_root/backend/Dockerfile"
        artifact_name="image.tar"
        ;;
    frontend)
        : "${NEXT_PUBLIC_API_URL:?set the exact public marketplace API URL}"
        : "${NEXT_PUBLIC_SUPABASE_URL:?set the exact public Supabase URL}"
        : "${NEXT_PUBLIC_SUPABASE_ANON_KEY:?set the public Supabase anonymous key}"
        dockerfile="$repo_root/frontend/Dockerfile"
        artifact_name="image.tar"
        ;;
    agent)
        : "${STADO_AGENT_BUILDER_IMAGE:?set a digest-pinned Rust builder image}"
        case "$STADO_AGENT_BUILDER_IMAGE" in
            *@sha256:*) ;;
            *) echo "STADO_AGENT_BUILDER_IMAGE must be pinned by sha256 digest"; false ;;
        esac
        dockerfile="$repo_root/agent/Dockerfile"
        artifact_name="wisent-agent"
        ;;
    *) echo "MARKETPLACE_COMPONENT must be backend, frontend, or agent"; false ;;
esac
build_tag="compute-marketplace-$component:stado-release"
work_dir="$(mktemp -d)"
archive="$work_dir/$artifact_name"
container_id=""
cleanup() {
    if [ -n "$container_id" ]; then
        docker rm "$container_id" >/dev/null
    fi
    rm -rf "$work_dir"
}
trap cleanup EXIT
auth_header="$work_dir/stado-auth-header"
umask go-rwx
printf 'Authorization: Bearer %s\n' "$stado_api_token" > "$auth_header"

if [ "$component" = agent ]; then
    docker build \
        --file "$dockerfile" \
        --build-arg "RUST_BUILDER_IMAGE=$STADO_AGENT_BUILDER_IMAGE" \
        --tag "$build_tag" \
        "$repo_root"
    container_id="$(docker create "$build_tag")"
    docker cp "$container_id:/wisent-agent" "$archive"
    docker rm "$container_id" >/dev/null
    container_id=""
elif [ "$component" = frontend ]; then
    docker build \
        --build-context "onboarding=$repo_root/../echo-web/packages/onboarding-web" \
        --file "$dockerfile" \
        --build-arg "NEXT_PUBLIC_API_URL=$NEXT_PUBLIC_API_URL" \
        --build-arg "NEXT_PUBLIC_SUPABASE_URL=$NEXT_PUBLIC_SUPABASE_URL" \
        --build-arg "NEXT_PUBLIC_SUPABASE_ANON_KEY=$NEXT_PUBLIC_SUPABASE_ANON_KEY" \
        --tag "$build_tag" \
        "$repo_root"
    docker save --output "$archive" "$build_tag"
else
    docker build \
        --file "$dockerfile" \
        --build-arg "STADO_RELEASE_URL=$STADO_RELEASE_URL" \
        --build-arg "STADO_RELEASE_SHA256=$STADO_RELEASE_SHA256" \
        --tag "$build_tag" \
        "$repo_root"
    docker save --output "$archive" "$build_tag"
fi

checksum_output="$(openssl dgst -sha256 "$archive")"
checksum="${checksum_output##* }"
release_uri="stado://releases/compute-marketplace/$component/sha256/$checksum/$artifact_name"
object_url="$stado_api_url/api/object?uri=$release_uri&if_absent=true"
release_url="$stado_api_url/api/release/object?uri=$release_uri"
if ! curl --fail --silent --show-error \
    --request PUT \
    --header "@$auth_header" \
    --header "Content-Type: application/octet-stream" \
    --data-binary "@$archive" \
    "$object_url" >/dev/null
then
    existing="$work_dir/existing-$artifact_name"
    curl --fail --silent --show-error "$release_url" --output "$existing"
    if ! cmp --silent "$archive" "$existing"; then
        echo "immutable marketplace release collision at $release_uri"
        false
    fi
fi

variable_component="$(printf '%s' "$component" | tr '[:lower:]' '[:upper:]')"
printf 'STADO_%s_RELEASE_URI=%s\n' "$variable_component" "$release_uri"
printf 'STADO_%s_RELEASE_SHA256=%s\n' "$variable_component" "$checksum"
