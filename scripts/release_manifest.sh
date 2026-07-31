#!/bin/sh
# Emit a canonical immutable release manifest for one platform directory.
set -eu

die() {
    printf 'FATAL: %s\n' "$*" > /dev/stderr
    false
}

RELEASE_DIR="${STADO_RELEASE_DIR:?set STADO_RELEASE_DIR}"
VERSION="${STADO_RELEASE_VERSION:?set STADO_RELEASE_VERSION}"
CHANNEL="${STADO_RELEASE_CHANNEL:?set STADO_RELEASE_CHANNEL}"
PLATFORM="${STADO_RELEASE_PLATFORM:?set STADO_RELEASE_PLATFORM}"
SOURCE_COMMIT="${STADO_RELEASE_SOURCE_COMMIT:?set STADO_RELEASE_SOURCE_COMMIT}"
SOURCE_REPOSITORY="${STADO_RELEASE_SOURCE_REPOSITORY:?set STADO_RELEASE_SOURCE_REPOSITORY}"
BUILT_AT="${STADO_RELEASE_BUILT_AT:?set STADO_RELEASE_BUILT_AT}"
MACHINE_SCHEMA="${STADO_MACHINE_SCHEMA_VERSION:?set STADO_MACHINE_SCHEMA_VERSION}"
CONFIG_SCHEMA="${STADO_CONFIG_SCHEMA_VERSION:?set STADO_CONFIG_SCHEMA_VERSION}"
STORAGE_SCHEMA="${STADO_STORAGE_SCHEMA_VERSION:?set STADO_STORAGE_SCHEMA_VERSION}"
MIN_AGENT_VERSION="${STADO_MIN_AGENT_VERSION:-$VERSION}"
LICENSE_FILE="${STADO_LICENSE_FILE:?set STADO_LICENSE_FILE}"
STABLE_INTEGRATIONS="${STADO_RELEASE_STABLE_INTEGRATIONS:?set STADO_RELEASE_STABLE_INTEGRATIONS}"
PREVIEW_INTEGRATIONS="${STADO_RELEASE_PREVIEW_INTEGRATIONS:?set STADO_RELEASE_PREVIEW_INTEGRATIONS}"

for command_name in jq openssl wc; do
    command -v "$command_name" >/dev/null || die "required command is unavailable: $command_name"
done

case "$CHANNEL" in
    nightly|candidate|stable) ;;
    *) die "release channel must be nightly, candidate, or stable" ;;
esac

artifacts_file="$RELEASE_DIR/.artifacts.json"
printf '[]\n' > "$artifacts_file"
for name in stado wc stado-coverage stado-fix stado-watchdog stado-mcp; do
    path="$RELEASE_DIR/$name"
    [ -f "$path" ] || die "release binary is missing: $name"
    digest="$(openssl dgst -sha256 "$path" | sed 's/^.*= //')"
    size="$(/usr/bin/wc -c < "$path" | tr -d '[:space:]')"
    updated="$RELEASE_DIR/.artifacts.next.json"
    jq \
        --arg name "$name" \
        --arg sha256 "$digest" \
        --argjson size_bytes "$size" \
        '. + [{"name": $name, "sha256": $sha256, "size_bytes": $size_bytes}]' \
        "$artifacts_file" > "$updated"
    mv "$updated" "$artifacts_file"
done

license_digest="$(openssl dgst -sha256 "$LICENSE_FILE" | sed 's/^.*= //')"
manifest="$RELEASE_DIR/release-manifest.json"
jq -cS -n \
    --arg contract "stado-release-manifest" \
    --arg product "stado" \
    --arg version "$VERSION" \
    --arg channel "$CHANNEL" \
    --arg platform "$PLATFORM" \
    --arg source_commit "$SOURCE_COMMIT" \
    --arg source_repository "$SOURCE_REPOSITORY" \
    --arg built_at "$BUILT_AT" \
    --arg minimum_agent_version "$MIN_AGENT_VERSION" \
    --arg license_file "LICENSE" \
    --arg license_sha256 "$license_digest" \
    --arg stable_integrations "$STABLE_INTEGRATIONS" \
    --arg preview_integrations "$PREVIEW_INTEGRATIONS" \
    --argjson machine_api "$MACHINE_SCHEMA" \
    --argjson configuration "$CONFIG_SCHEMA" \
    --argjson storage_layout "$STORAGE_SCHEMA" \
    --slurpfile artifacts "$artifacts_file" \
    '{
        "artifacts": $artifacts[0],
        "built_at": $built_at,
        "channel": $channel,
        "contract": $contract,
        "license": {"file": $license_file, "sha256": $license_sha256},
        "minimum_agent_version": $minimum_agent_version,
        "platform": $platform,
        "stable_integrations": ($stable_integrations | split(",") | map(select(length != ("" | length)))),
        "preview_integrations": ($preview_integrations | split(",") | map(select(length != ("" | length)))),
        "product": $product,
        "schema_versions": {
            "configuration": $configuration,
            "machine_api": $machine_api,
            "storage_layout": $storage_layout
        },
        "source_commit": $source_commit,
        "source_repository": $source_repository,
        "version": $version
    }' > "$manifest"
rm "$artifacts_file"
printf '%s\n' "$manifest"
